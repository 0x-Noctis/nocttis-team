use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;

use super::policy::Role;

const MAX_HEADER_BYTES: usize = 4 * 1024;
const MAX_CONTENT_BYTES: usize = 64 * 1024;
const MAX_SKILLS: usize = 128;
const MAX_WORKER_SKILLS: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SkillMetadata {
    pub id: String,
    pub roles: Vec<String>,
    pub triggers: Vec<String>,
    pub summary: String,
    pub estimated_tokens: usize,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillError {
    InvalidDirectory,
    InvalidMetadata,
    InvalidId,
    DuplicateId,
    UnsafePath,
    TooLarge,
    NotFound,
    Forbidden,
    BudgetExceeded,
    Changed,
    Io,
}

pub struct SkillRegistry {
    root: PathBuf,
    entries: BTreeMap<String, (SkillMetadata, PathBuf)>,
}

impl SkillRegistry {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, SkillError> {
        let root = root.as_ref();
        if fs::symlink_metadata(root)
            .map_err(|_| SkillError::InvalidDirectory)?
            .file_type()
            .is_symlink()
        {
            return Err(SkillError::UnsafePath);
        }
        let root = root
            .canonicalize()
            .map_err(|_| SkillError::InvalidDirectory)?;
        if !root.is_dir() {
            return Err(SkillError::InvalidDirectory);
        }
        let mut registry = Self {
            root,
            entries: BTreeMap::new(),
        };
        registry.reload()?;
        Ok(registry)
    }

    /// Mengganti indeks secara atomik agar kegagalan reload tidak menghapus skill lama.
    pub fn reload(&mut self) -> Result<(), SkillError> {
        self.check_root()?;
        let mut entries = BTreeMap::new();
        let mut pending = vec![self.root.clone()];
        while let Some(directory) = pending.pop() {
            if fs::symlink_metadata(&directory)
                .map_err(|_| SkillError::Io)?
                .file_type()
                .is_symlink()
                || directory.canonicalize().map_err(|_| SkillError::Io)? != directory
            {
                return Err(SkillError::UnsafePath);
            }
            for item in fs::read_dir(directory).map_err(|_| SkillError::Io)? {
                let path = item.map_err(|_| SkillError::Io)?.path();
                let kind = fs::symlink_metadata(&path)
                    .map_err(|_| SkillError::Io)?
                    .file_type();
                if kind.is_symlink() {
                    return Err(SkillError::UnsafePath);
                }
                let relative = path
                    .strip_prefix(&self.root)
                    .map_err(|_| SkillError::UnsafePath)?;
                check_path(relative)?;
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "md") {
                    if entries.len() == MAX_SKILLS {
                        return Err(SkillError::TooLarge);
                    }
                    let path = self.safe_file(relative)?;
                    let metadata = parse_header(&read_bounded(&path, MAX_HEADER_BYTES, true)?)?.0;
                    let id = metadata.id.clone();
                    if entries
                        .insert(id, (metadata, relative.to_path_buf()))
                        .is_some()
                    {
                        return Err(SkillError::DuplicateId);
                    }
                }
            }
        }
        self.entries = entries;
        Ok(())
    }

    /// Lead menerima indeks metadata tanpa path maupun isi Markdown.
    pub fn metadata(&self) -> Vec<SkillMetadata> {
        self.entries
            .values()
            .map(|(metadata, _)| metadata.clone())
            .collect()
    }

    pub fn match_worker(
        &self,
        task_role: &str,
        triggers: &[&str],
        budget: usize,
    ) -> Vec<SkillMetadata> {
        let mut remaining = budget;
        self.entries
            .values()
            .filter_map(|(metadata, _)| {
                if metadata.roles.iter().any(|role| role == task_role)
                    && metadata.triggers.iter().any(|trigger| {
                        triggers
                            .iter()
                            .any(|value| trigger.eq_ignore_ascii_case(value))
                    })
                    && metadata.estimated_tokens <= remaining
                {
                    remaining -= metadata.estimated_tokens;
                    Some(metadata.clone())
                } else {
                    None
                }
            })
            .take(MAX_WORKER_SKILLS)
            .collect()
    }

    /// Konten hanya dapat diminta Worker; metadata diperiksa ulang saat file berubah.
    pub fn load(
        &self,
        role: Role,
        id: &str,
        task_role: &str,
        triggers: &[&str],
        budget: usize,
    ) -> Result<String, SkillError> {
        if role != Role::Worker {
            return Err(SkillError::Forbidden);
        }
        let (metadata, relative) = self.entries.get(id).ok_or(SkillError::NotFound)?;
        if !metadata.roles.iter().any(|value| value == task_role)
            || !metadata.triggers.iter().any(|trigger| {
                triggers
                    .iter()
                    .any(|value| trigger.eq_ignore_ascii_case(value))
            })
        {
            return Err(SkillError::Forbidden);
        }
        if !self
            .match_worker(task_role, triggers, budget)
            .iter()
            .any(|skill| skill.id == id)
        {
            return Err(SkillError::BudgetExceeded);
        }
        let path = self.safe_file(relative)?;
        let bytes = read_bounded(&path, MAX_CONTENT_BYTES, false)?;
        let (current, offset) = parse_header(&bytes)?;
        if current != *metadata {
            return Err(SkillError::Changed);
        }
        let content =
            std::str::from_utf8(&bytes[offset..]).map_err(|_| SkillError::InvalidMetadata)?;
        if content.contains('\0') {
            return Err(SkillError::InvalidMetadata);
        }
        if content.len().div_ceil(4) > budget.min(metadata.estimated_tokens) {
            return Err(SkillError::BudgetExceeded);
        }
        Ok(content.to_owned())
    }

    fn check_root(&self) -> Result<(), SkillError> {
        let kind = fs::symlink_metadata(&self.root)
            .map_err(|_| SkillError::Io)?
            .file_type();
        if kind.is_symlink() || !kind.is_dir() {
            return Err(SkillError::UnsafePath);
        }
        if self.root.canonicalize().map_err(|_| SkillError::Io)? != self.root {
            return Err(SkillError::UnsafePath);
        }
        Ok(())
    }

    fn safe_file(&self, relative: &Path) -> Result<PathBuf, SkillError> {
        self.check_root()?;
        check_path(relative)?;
        let mut path = self.root.clone();
        for component in relative.components() {
            path.push(component);
            if fs::symlink_metadata(&path)
                .map_err(|_| SkillError::Io)?
                .file_type()
                .is_symlink()
            {
                return Err(SkillError::UnsafePath);
            }
        }
        let canonical = path.canonicalize().map_err(|_| SkillError::Io)?;
        if !canonical.starts_with(&self.root) || !canonical.is_file() {
            return Err(SkillError::UnsafePath);
        }
        Ok(canonical)
    }
}

fn check_path(path: &Path) -> Result<(), SkillError> {
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(SkillError::UnsafePath);
        };
        let name = name
            .to_str()
            .ok_or(SkillError::UnsafePath)?
            .to_ascii_lowercase();
        if name == ".env"
            || name.starts_with(".env.")
            || matches!(
                name.as_str(),
                ".netrc"
                    | ".npmrc"
                    | ".pypirc"
                    | "credentials"
                    | "credentials.json"
                    | "id_rsa"
                    | "id_ed25519"
            )
            || matches!(
                Path::new(&name).extension().and_then(|ext| ext.to_str()),
                Some("pem" | "key" | "p12" | "pfx")
            )
        {
            return Err(SkillError::UnsafePath);
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, maximum: usize, header_only: bool) -> Result<Vec<u8>, SkillError> {
    let file = File::open(path).map_err(|_| SkillError::Io)?;
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SkillError::Io)?;
    if bytes.len() > maximum {
        if !header_only {
            return Err(SkillError::TooLarge);
        }
        bytes.truncate(maximum);
    }
    Ok(bytes)
}

fn parse_header(bytes: &[u8]) -> Result<(SkillMetadata, usize), SkillError> {
    let prefix = bytes.get(..4).ok_or(SkillError::InvalidMetadata)?;
    if prefix != b"---\n" {
        return Err(SkillError::InvalidMetadata);
    }
    let end = bytes[4..]
        .windows(5)
        .position(|window| window == b"\n---\n")
        .map(|position| position + 4)
        .ok_or(SkillError::InvalidMetadata)?;
    let header = std::str::from_utf8(&bytes[4..end]).map_err(|_| SkillError::InvalidMetadata)?;
    let mut fields = BTreeMap::new();
    for line in header.lines() {
        let (key, value) = line.split_once(':').ok_or(SkillError::InvalidMetadata)?;
        if !matches!(
            key,
            "id" | "roles" | "triggers" | "summary" | "estimated_tokens" | "version"
        ) || fields.insert(key, value.trim()).is_some()
        {
            return Err(SkillError::InvalidMetadata);
        }
    }
    let field = |key| fields.get(key).copied().ok_or(SkillError::InvalidMetadata);
    let id = field("id")?;
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || id.starts_with('-')
        || id.ends_with('-')
    {
        return Err(SkillError::InvalidId);
    }
    let summary = field("summary")?;
    if summary.is_empty() || summary.len() > 256 || summary.chars().any(char::is_control) {
        return Err(SkillError::InvalidMetadata);
    }
    let estimated_tokens = field("estimated_tokens")?
        .parse()
        .map_err(|_| SkillError::InvalidMetadata)?;
    if estimated_tokens == 0 || estimated_tokens > MAX_CONTENT_BYTES.div_ceil(4) {
        return Err(SkillError::InvalidMetadata);
    }
    let version = field("version")?
        .parse()
        .map_err(|_| SkillError::InvalidMetadata)?;
    if version == 0 {
        return Err(SkillError::InvalidMetadata);
    }
    Ok((
        SkillMetadata {
            id: id.to_owned(),
            roles: parse_list(field("roles")?)?,
            triggers: parse_list(field("triggers")?)?,
            summary: summary.to_owned(),
            estimated_tokens,
            version,
        },
        end + 5,
    ))
}

fn parse_list(value: &str) -> Result<Vec<String>, SkillError> {
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .ok_or(SkillError::InvalidMetadata)?;
    let items: Vec<_> = inner.split(',').map(str::trim).collect();
    if items.is_empty()
        || items.len() > 16
        || items.iter().any(|item| {
            item.is_empty()
                || item.len() > 64
                || !item.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'
                        || byte == b'-'
                })
        })
    {
        return Err(SkillError::InvalidMetadata);
    }
    Ok(items.into_iter().map(str::to_owned).collect())
}
