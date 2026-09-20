use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
};

const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitOperation {
    ValidateRepository,
    ValidateBaseCommit,
    ValidateWorktree,
    ValidateBranch,
    ValidateAncestry,
    CreateWorktree,
    ReadHead,
    ReadStatus,
    ReadDiff,
    ApplyPatch,
    CleanupWorktree,
    PruneWorktrees,
    CleanupBranch,
}

impl fmt::Display for GitOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ValidateRepository => "repository validation",
            Self::ValidateBaseCommit => "base commit validation",
            Self::ValidateWorktree => "worktree validation",
            Self::ValidateBranch => "worktree branch validation",
            Self::ValidateAncestry => "worktree ancestry validation",
            Self::CreateWorktree => "worktree creation",
            Self::ReadHead => "HEAD read",
            Self::ReadStatus => "status read",
            Self::ReadDiff => "diff read",
            Self::ApplyPatch => "patch application",
            Self::CleanupWorktree => "worktree cleanup",
            Self::PruneWorktrees => "worktree prune",
            Self::CleanupBranch => "branch cleanup",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandStatus {
    Code(i32),
    Terminated,
}

impl fmt::Display for CommandStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(formatter, "{code}"),
            Self::Terminated => formatter.write_str("terminated"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

impl fmt::Display for OutputStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

#[derive(Debug)]
pub enum GitError {
    InvalidInput(&'static str),
    Io,
    CommandFailed {
        operation: GitOperation,
        status: CommandStatus,
    },
    OutputTooLarge {
        operation: GitOperation,
        stream: OutputStream,
    },
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid git input: {message}"),
            Self::Io => formatter.write_str("git operation failed due to an I/O error"),
            Self::CommandFailed { operation, status } => {
                write!(formatter, "git {operation} failed (status {status})")
            }
            Self::OutputTooLarge { operation, stream } => {
                write!(formatter, "git {operation} {stream} exceeded output limit")
            }
        }
    }
}

impl std::error::Error for GitError {}

impl From<io::Error> for GitError {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Worktree {
    path: PathBuf,
    branch: String,
    base_commit: String,
}

impl Worktree {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn base_commit(&self) -> &str {
        &self.base_commit
    }
}

#[derive(Debug)]
pub struct GitWorktreeManager {
    repository_root: PathBuf,
    worktree_root: PathBuf,
}

impl GitWorktreeManager {
    pub fn new(
        repository: impl AsRef<Path>,
        worktree_root: impl AsRef<Path>,
    ) -> Result<Self, GitError> {
        let repository = fs::canonicalize(repository)
            .map_err(|_| GitError::InvalidInput("repository does not exist"))?;
        let output = run(
            Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["rev-parse", "--show-toplevel"]),
            None,
            GitOperation::ValidateRepository,
        )?;
        let reported_root = path_output(&output)?;
        let repository_root = fs::canonicalize(reported_root)
            .map_err(|_| GitError::InvalidInput("repository root is invalid"))?;

        fs::create_dir_all(worktree_root.as_ref())?;
        let worktree_root = fs::canonicalize(worktree_root)?;

        Ok(Self {
            repository_root,
            worktree_root,
        })
    }

    pub fn repository_root(&self) -> &Path {
        &self.repository_root
    }

    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    pub fn create(
        &self,
        task_id: &str,
        branch: &str,
        base_commit: &str,
    ) -> Result<Worktree, GitError> {
        validate_component(task_id, "task ID")?;
        validate_component(branch, "branch")?;
        validate_component(base_commit, "base commit")?;
        let base_commit = self.resolve_commit(base_commit)?;
        let path = self.worktree_root.join(task_id);
        if path.exists() {
            return Err(GitError::InvalidInput("worktree path already exists"));
        }

        run(
            Command::new("git")
                .arg("-C")
                .arg(&self.repository_root)
                .args(["worktree", "add", "-b", branch])
                .arg(&path)
                .arg(&base_commit),
            None,
            GitOperation::CreateWorktree,
        )?;
        let path = fs::canonicalize(path)?;
        if path.parent() != Some(self.worktree_root.as_path()) {
            return Err(GitError::InvalidInput("worktree escaped configured root"));
        }

        Ok(Worktree {
            path,
            branch: branch.to_owned(),
            base_commit,
        })
    }

    pub fn open(
        &self,
        task_id: &str,
        branch: &str,
        base_commit: &str,
    ) -> Result<Worktree, GitError> {
        validate_component(task_id, "task ID")?;
        validate_component(branch, "branch")?;
        validate_component(base_commit, "base commit")?;
        let base_commit = self.resolve_commit(base_commit)?;
        let path = fs::canonicalize(self.worktree_root.join(task_id))
            .map_err(|_| GitError::InvalidInput("worktree does not exist"))?;
        if path.parent() != Some(self.worktree_root.as_path()) {
            return Err(GitError::InvalidInput("worktree escaped configured root"));
        }
        self.validate_registered_worktree(&path)?;

        let actual_branch = text_output(&run(
            Command::new("git").arg("-C").arg(&path).args([
                "symbolic-ref",
                "--quiet",
                "--short",
                "HEAD",
            ]),
            None,
            GitOperation::ValidateBranch,
        )?)?;
        if actual_branch != branch {
            return Err(GitError::InvalidInput("worktree branch does not match"));
        }
        if let Err(error) = run(
            Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(["merge-base", "--is-ancestor"])
                .arg(&base_commit)
                .arg("HEAD"),
            None,
            GitOperation::ValidateAncestry,
        ) {
            return match error {
                GitError::CommandFailed { .. } => {
                    Err(GitError::InvalidInput("base commit is not an ancestor"))
                }
                error => Err(error),
            };
        }

        Ok(Worktree {
            path,
            branch: branch.to_owned(),
            base_commit,
        })
    }

    pub fn head(&self, worktree: &Worktree) -> Result<String, GitError> {
        self.ensure_worktree(worktree)?;
        let output = run(
            Command::new("git")
                .arg("-C")
                .arg(&worktree.path)
                .args(["rev-parse", "HEAD"]),
            None,
            GitOperation::ReadHead,
        )?;
        text_output(&output)
    }

    pub fn status(&self, worktree: &Worktree) -> Result<String, GitError> {
        self.ensure_worktree(worktree)?;
        let output = run(
            Command::new("git").arg("-C").arg(&worktree.path).args([
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ]),
            None,
            GitOperation::ReadStatus,
        )?;
        String::from_utf8(output).map_err(|_| GitError::InvalidInput("git status was not UTF-8"))
    }

    pub fn diff_binary(&self, worktree: &Worktree) -> Result<Vec<u8>, GitError> {
        self.ensure_worktree(worktree)?;
        run(
            Command::new("git")
                .arg("-C")
                .arg(&worktree.path)
                .args(["diff", "--binary", "--no-ext-diff"])
                .arg(&worktree.base_commit)
                .arg("--"),
            None,
            GitOperation::ReadDiff,
        )
    }

    pub fn apply_patch(&self, worktree: &Worktree, patch: &[u8]) -> Result<(), GitError> {
        self.ensure_worktree(worktree)?;
        if patch.len() > OUTPUT_LIMIT {
            return Err(GitError::InvalidInput("patch exceeds size limit"));
        }
        run(
            Command::new("git").arg("-C").arg(&worktree.path).args([
                "apply",
                "--recount",
                "--whitespace=nowarn",
                "-",
            ]),
            Some(patch),
            GitOperation::ApplyPatch,
        )?;
        Ok(())
    }

    pub fn cleanup(&self, worktree: &Worktree) -> Result<(), GitError> {
        validate_component(&worktree.branch, "branch")?;
        if worktree.path.parent() != Some(self.worktree_root.as_path()) {
            return Err(GitError::InvalidInput("worktree escaped configured root"));
        }
        if worktree.path.exists() {
            run(
                Command::new("git")
                    .arg("-C")
                    .arg(&self.repository_root)
                    .args(["worktree", "remove", "--force"])
                    .arg(&worktree.path),
                None,
                GitOperation::CleanupWorktree,
            )?;
        } else {
            run(
                Command::new("git")
                    .arg("-C")
                    .arg(&self.repository_root)
                    .args(["worktree", "prune"]),
                None,
                GitOperation::PruneWorktrees,
            )?;
        }
        let branch_exists = Command::new("git")
            .arg("-C")
            .arg(&self.repository_root)
            .args(["show-ref", "--verify", "--quiet"])
            .arg(format!("refs/heads/{}", worktree.branch))
            .status()?;
        if branch_exists.success() {
            run(
                Command::new("git")
                    .arg("-C")
                    .arg(&self.repository_root)
                    .args(["branch", "-D", "--"])
                    .arg(&worktree.branch),
                None,
                GitOperation::CleanupBranch,
            )?;
        }
        Ok(())
    }

    fn resolve_commit(&self, commit: &str) -> Result<String, GitError> {
        let output = run(
            Command::new("git")
                .arg("-C")
                .arg(&self.repository_root)
                .args(["rev-parse", "--verify", "--end-of-options"])
                .arg(format!("{commit}^{{commit}}")),
            None,
            GitOperation::ValidateBaseCommit,
        )?;
        text_output(&output)
    }

    fn validate_registered_worktree(&self, path: &Path) -> Result<(), GitError> {
        let output = run(
            Command::new("git")
                .arg("-C")
                .arg(&self.repository_root)
                .args(["worktree", "list", "--porcelain", "-z"]),
            None,
            GitOperation::ValidateWorktree,
        )?;
        let registered = output
            .split(|byte| *byte == 0)
            .filter_map(|field| field.strip_prefix(b"worktree "))
            .filter_map(|value| std::str::from_utf8(value).ok())
            .filter_map(|value| fs::canonicalize(value).ok())
            .any(|registered| registered == path);
        if !registered {
            return Err(GitError::InvalidInput("worktree is not registered"));
        }

        let root = path_output(&run(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["rev-parse", "--show-toplevel"]),
            None,
            GitOperation::ValidateWorktree,
        )?)?;
        if fs::canonicalize(root).map_err(|_| GitError::InvalidInput("worktree root is invalid"))?
            != path
        {
            return Err(GitError::InvalidInput("worktree root does not match"));
        }

        let repository_common = self.common_git_directory(&self.repository_root)?;
        let worktree_common = self.common_git_directory(path)?;
        if repository_common != worktree_common {
            return Err(GitError::InvalidInput("worktree repository does not match"));
        }
        Ok(())
    }

    fn common_git_directory(&self, path: &Path) -> Result<PathBuf, GitError> {
        let common = path_output(&run(
            Command::new("git").arg("-C").arg(path).args([
                "rev-parse",
                "--path-format=absolute",
                "--git-common-dir",
            ]),
            None,
            GitOperation::ValidateWorktree,
        )?)?;
        fs::canonicalize(common).map_err(|_| GitError::InvalidInput("git directory is invalid"))
    }

    fn ensure_worktree(&self, worktree: &Worktree) -> Result<(), GitError> {
        let path = fs::canonicalize(&worktree.path)
            .map_err(|_| GitError::InvalidInput("worktree does not exist"))?;
        if path.parent() != Some(self.worktree_root.as_path()) {
            return Err(GitError::InvalidInput("worktree escaped configured root"));
        }
        Ok(())
    }
}

fn validate_component(value: &str, name: &'static str) -> Result<(), GitError> {
    if value.is_empty()
        || value.starts_with('-')
        || Path::new(value).is_absolute()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(GitError::InvalidInput(name));
    }
    Ok(())
}

fn run(
    command: &mut Command,
    stdin: Option<&[u8]>,
    operation: GitOperation,
) -> Result<Vec<u8>, GitError> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr"))?;
    let stdout_reader = thread::spawn(move || read_limited(stdout));
    let stderr_reader = thread::spawn(move || read_limited(stderr));
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing stdin"))?
            .write_all(input)?;
    }
    let status = child.wait()?;
    let (stdout, stdout_overflow) = join_reader(stdout_reader)?;
    let (_, stderr_overflow) = join_reader(stderr_reader)?;
    command_result(status, stdout, stdout_overflow, stderr_overflow, operation)
}

fn read_limited(mut reader: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut overflow = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = OUTPUT_LIMIT.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(remaining)]);
        overflow |= read > remaining;
    }
    Ok((kept, overflow))
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<(Vec<u8>, bool)>>,
) -> Result<(Vec<u8>, bool), GitError> {
    reader
        .join()
        .map_err(|_| io::Error::other("git output reader panicked"))?
        .map_err(|_| GitError::Io)
}

fn command_result(
    status: ExitStatus,
    stdout: Vec<u8>,
    stdout_overflow: bool,
    stderr_overflow: bool,
    operation: GitOperation,
) -> Result<Vec<u8>, GitError> {
    if stderr_overflow {
        return Err(GitError::OutputTooLarge {
            operation,
            stream: OutputStream::Stderr,
        });
    }
    if stdout_overflow {
        return Err(GitError::OutputTooLarge {
            operation,
            stream: OutputStream::Stdout,
        });
    }
    if !status.success() {
        return Err(GitError::CommandFailed {
            operation,
            status: status
                .code()
                .map_or(CommandStatus::Terminated, CommandStatus::Code),
        });
    }
    Ok(stdout)
}

fn text_output(output: &[u8]) -> Result<String, GitError> {
    String::from_utf8(output.to_vec())
        .map(|value| value.trim().to_owned())
        .map_err(|_| GitError::InvalidInput("git output was not UTF-8"))
}

fn path_output(output: &[u8]) -> Result<PathBuf, GitError> {
    text_output(output).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_overflow_has_typed_error_without_content() {
        let (stderr, overflow) =
            read_limited(io::Cursor::new(vec![b'x'; OUTPUT_LIMIT + 1])).unwrap();
        assert_eq!(stderr.len(), OUTPUT_LIMIT);
        assert!(overflow);
        let status = Command::new("git").arg("--version").status().unwrap();
        let error = command_result(
            status,
            Vec::new(),
            false,
            overflow,
            GitOperation::ApplyPatch,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            GitError::OutputTooLarge {
                operation: GitOperation::ApplyPatch,
                stream: OutputStream::Stderr
            }
        ));
        assert_eq!(
            error.to_string(),
            "git patch application stderr exceeded output limit"
        );
    }
}
