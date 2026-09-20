use std::{
    fmt, fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{domain::task::TaskContract, store::artifact::ArtifactStore};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextSourceKind {
    TaskContract,
    Artifact,
    SourceFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextSource {
    pub kind: ContextSourceKind,
    pub reference: String,
    pub reason: String,
    pub original_bytes: usize,
    pub included_bytes: usize,
    pub truncation_reason: Option<TruncationReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TruncationReason {
    Bytes,
    Tokens,
    CommandOutput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextEntry {
    pub content: String,
    pub source: ContextSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltContext {
    pub entries: Vec<ContextEntry>,
    pub bytes: usize,
    pub estimated_tokens: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ContextRequest {
    pub search_terms: Vec<String>,
    pub excerpts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextLimits {
    pub max_bytes: usize,
    pub max_tokens: usize,
    pub max_file_bytes: usize,
    pub max_command_output_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextError {
    InvalidRepository,
    InvalidPath,
    PathNotAllowed,
    Symlink,
    BinaryFile,
    FileTooLarge,
    InvalidArtifactReference,
    ArtifactUnavailable,
    InvalidSearch,
    SearchFailed,
    Io(&'static str),
}

impl fmt::Display for ContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRepository => "invalid repository root",
            Self::InvalidPath => "invalid context path",
            Self::PathNotAllowed => "context path is not allowed",
            Self::Symlink => "context path contains a symlink",
            Self::BinaryFile => "binary context source rejected",
            Self::FileTooLarge => "context source exceeds file size limit",
            Self::InvalidArtifactReference => "invalid artifact reference",
            Self::ArtifactUnavailable => "artifact unavailable",
            Self::InvalidSearch => "invalid context search",
            Self::SearchFailed => "context search failed",
            Self::Io(operation) => operation,
        })
    }
}

impl std::error::Error for ContextError {}

pub struct ContextBuilder<'a> {
    repository_root: PathBuf,
    artifact_store: &'a ArtifactStore,
    limits: ContextLimits,
}

impl<'a> ContextBuilder<'a> {
    pub fn new(
        repository_root: impl AsRef<Path>,
        artifact_store: &'a ArtifactStore,
        limits: ContextLimits,
    ) -> Result<Self, ContextError> {
        if limits.max_bytes == 0
            || limits.max_tokens == 0
            || limits.max_file_bytes == 0
            || limits.max_command_output_bytes == 0
        {
            return Err(ContextError::InvalidRepository);
        }
        let repository_root = repository_root
            .as_ref()
            .canonicalize()
            .map_err(|_| ContextError::InvalidRepository)?;
        if !repository_root.is_dir() {
            return Err(ContextError::InvalidRepository);
        }
        Ok(Self {
            repository_root,
            artifact_store,
            limits,
        })
    }

    pub fn build(
        &self,
        contract: &TaskContract,
        request: &ContextRequest,
    ) -> Result<BuiltContext, ContextError> {
        let task_token_limit =
            usize::try_from(contract.limits.max_input_tokens.get()).unwrap_or(usize::MAX);
        let mut budget = Budget::new(
            self.limits.max_bytes,
            self.limits.max_tokens.min(task_token_limit),
        );
        let mut entries = Vec::new();
        let task = serde_json::to_string(contract)
            .map_err(|_| ContextError::Io("serialize task contract"))?;
        budget.push(
            &mut entries,
            ContextSourceKind::TaskContract,
            contract.id.as_str().to_owned(),
            "task contract".to_owned(),
            task,
            None,
        );

        let mut artifact_refs: Vec<_> = contract
            .context_refs
            .iter()
            .map(|reference| reference.as_str())
            .collect();
        artifact_refs.sort_unstable();
        for reference in artifact_refs {
            let artifact_id = reference
                .strip_prefix("artifact://")
                .filter(|id| valid_artifact_id(id))
                .ok_or(ContextError::InvalidArtifactReference)?;
            let bytes = self
                .artifact_store
                .read(artifact_id)
                .map_err(|_| ContextError::ArtifactUnavailable)?;
            let content = text(bytes, self.limits.max_file_bytes)?;
            budget.push(
                &mut entries,
                ContextSourceKind::Artifact,
                reference.to_owned(),
                "referenced by task".to_owned(),
                content,
                None,
            );
        }

        let mut sources: Vec<(String, String, Option<TruncationReason>)> = request
            .excerpts
            .iter()
            .map(|path| (path.clone(), "requested excerpt".to_owned(), None))
            .collect();
        let mut terms = request.search_terms.clone();
        terms.sort();
        terms.dedup();
        for term in terms {
            validate_search(&term)?;
            let (paths, truncated) = self.search(contract, &term)?;
            for path in paths {
                sources.push((
                    path,
                    format!("matched search term {term:?}"),
                    truncated.then_some(TruncationReason::CommandOutput),
                ));
            }
        }
        sources.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        sources.dedup_by(|left, right| left.0 == right.0);

        for (relative, reason, command_truncation) in sources {
            let path = self.resolve_source(contract, &relative)?;
            let bytes = fs::read(path).map_err(|_| ContextError::Io("read context source"))?;
            let content = text(bytes, self.limits.max_file_bytes)?;
            budget.push(
                &mut entries,
                ContextSourceKind::SourceFile,
                relative,
                reason,
                content,
                command_truncation,
            );
        }
        Ok(BuiltContext {
            entries,
            bytes: budget.bytes,
            estimated_tokens: budget.tokens,
        })
    }

    fn search(
        &self,
        contract: &TaskContract,
        term: &str,
    ) -> Result<(Vec<String>, bool), ContextError> {
        let mut command = Command::new("rg");
        command
            .arg("--files-with-matches")
            .arg("--fixed-strings")
            .arg("--no-messages")
            .arg("--color=never");
        for allowed in &contract.allowed_paths {
            command.arg("--glob").arg(normalized(allowed.as_str()));
        }
        let mut child = command
            .arg("--")
            .arg(term)
            .arg(".")
            .current_dir(&self.repository_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| ContextError::SearchFailed)?;
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or(ContextError::SearchFailed)?
            .take(self.limits.max_command_output_bytes as u64 + 1)
            .read_to_end(&mut output)
            .map_err(|_| ContextError::SearchFailed)?;
        let truncated = output.len() > self.limits.max_command_output_bytes;
        if truncated {
            output.truncate(self.limits.max_command_output_bytes);
            output.truncate(
                output
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |position| position + 1),
            );
            let _ = child.kill();
        }
        let status = child.wait().map_err(|_| ContextError::SearchFailed)?;
        if !truncated && !matches!(status.code(), Some(0 | 1)) {
            return Err(ContextError::SearchFailed);
        }
        let output = std::str::from_utf8(&output).map_err(|_| ContextError::SearchFailed)?;
        let mut paths = Vec::new();
        for line in output.lines() {
            let relative = line.strip_prefix("./").unwrap_or(line);
            self.resolve_source(contract, relative)?;
            paths.push(relative.to_owned());
        }
        paths.sort();
        paths.dedup();
        Ok((paths, truncated))
    }

    fn resolve_source(
        &self,
        contract: &TaskContract,
        relative: &str,
    ) -> Result<PathBuf, ContextError> {
        validate_relative(relative)?;
        let relative = normalized(relative);
        if is_secret_path(&relative) {
            return Err(ContextError::PathNotAllowed);
        }
        if !contract
            .allowed_paths
            .iter()
            .any(|allowed| glob_matches(&normalized(allowed.as_str()), &relative))
        {
            return Err(ContextError::PathNotAllowed);
        }
        let mut path = self.repository_root.clone();
        for component in Path::new(&relative).components() {
            let Component::Normal(component) = component else {
                return Err(ContextError::InvalidPath);
            };
            path.push(component);
            let metadata = fs::symlink_metadata(&path).map_err(|_| ContextError::InvalidPath)?;
            if metadata.file_type().is_symlink() {
                return Err(ContextError::Symlink);
            }
        }
        let canonical = path.canonicalize().map_err(|_| ContextError::InvalidPath)?;
        if !canonical.starts_with(&self.repository_root) || !canonical.is_file() {
            return Err(ContextError::InvalidPath);
        }
        Ok(canonical)
    }
}

struct Budget {
    max_bytes: usize,
    max_tokens: usize,
    bytes: usize,
    tokens: usize,
}

impl Budget {
    fn new(max_bytes: usize, max_tokens: usize) -> Self {
        Self {
            max_bytes,
            max_tokens,
            bytes: 0,
            tokens: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        entries: &mut Vec<ContextEntry>,
        kind: ContextSourceKind,
        reference: String,
        reason: String,
        content: String,
        prior_truncation: Option<TruncationReason>,
    ) {
        let original_bytes = content.len();
        let byte_room = self.max_bytes.saturating_sub(self.bytes);
        let token_room = self.max_tokens.saturating_sub(self.tokens);
        let token_byte_room = token_room.saturating_mul(4);
        let permitted = original_bytes.min(byte_room).min(token_byte_room);
        let included = floor_char_boundary(&content, permitted);
        let truncation_reason = if included < original_bytes {
            Some(if byte_room <= token_byte_room {
                TruncationReason::Bytes
            } else {
                TruncationReason::Tokens
            })
        } else {
            prior_truncation
        };
        let content = content[..included].to_owned();
        let tokens = estimate_tokens(content.len());
        self.bytes += content.len();
        self.tokens += tokens;
        entries.push(ContextEntry {
            content,
            source: ContextSource {
                kind,
                reference,
                reason,
                original_bytes,
                included_bytes: included,
                truncation_reason,
            },
        });
    }
}

pub fn estimate_tokens(bytes: usize) -> usize {
    bytes.div_ceil(4)
}

fn text(bytes: Vec<u8>, maximum: usize) -> Result<String, ContextError> {
    if bytes.len() > maximum {
        return Err(ContextError::FileTooLarge);
    }
    if bytes
        .iter()
        .any(|byte| byte.is_ascii_control() && !matches!(byte, b'\n' | b'\r' | b'\t'))
    {
        return Err(ContextError::BinaryFile);
    }
    String::from_utf8(bytes).map_err(|_| ContextError::BinaryFile)
}

fn validate_search(value: &str) -> Result<(), ContextError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(ContextError::InvalidSearch);
    }
    Ok(())
}

fn validate_relative(value: &str) -> Result<(), ContextError> {
    let windows_absolute = value
        .as_bytes()
        .get(1)
        .is_some_and(|separator| *separator == b':');
    if value.is_empty()
        || windows_absolute
        || value.chars().any(char::is_control)
        || value.contains('\\')
    {
        return Err(ContextError::InvalidPath);
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(ContextError::InvalidPath);
    }
    Ok(())
}

fn normalized(value: &str) -> String {
    value.replace('\\', "/")
}

fn glob_matches(pattern: &str, path: &str) -> bool {
    glob_bytes(pattern.as_bytes(), path.as_bytes())
}

fn glob_bytes(pattern: &[u8], path: &[u8]) -> bool {
    match pattern.first() {
        None => path.is_empty(),
        Some(b'*') if pattern.get(1) == Some(&b'*') => {
            let rest = &pattern[2..];
            glob_bytes(rest, path)
                || (!path.is_empty() && glob_bytes(pattern, &path[1..]))
                || (rest.first() == Some(&b'/') && glob_bytes(&rest[1..], path))
        }
        Some(b'*') => {
            glob_bytes(&pattern[1..], path)
                || (!path.is_empty() && path[0] != b'/' && glob_bytes(pattern, &path[1..]))
        }
        Some(b'?') => !path.is_empty() && path[0] != b'/' && glob_bytes(&pattern[1..], &path[1..]),
        Some(byte) => !path.is_empty() && *byte == path[0] && glob_bytes(&pattern[1..], &path[1..]),
    }
}

fn is_secret_path(path: &str) -> bool {
    Path::new(path).components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        name == ".env"
            || name.starts_with(".env.")
            || matches!(
                name.as_str(),
                "credentials" | "credentials.json" | "id_rsa" | "id_ed25519"
            )
    })
}

fn valid_artifact_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !matches!(value, "." | "..")
}

fn floor_char_boundary(value: &str, maximum: usize) -> usize {
    let mut boundary = maximum.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}
