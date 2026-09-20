use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
};

const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Debug)]
pub enum GitError {
    InvalidInput(&'static str),
    Io(io::Error),
    CommandFailed {
        operation: &'static str,
        status: Option<i32>,
        stderr: String,
    },
    OutputTooLarge(&'static str),
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(formatter, "invalid git input: {message}"),
            Self::Io(_) => formatter.write_str("git operation failed due to an I/O error"),
            Self::CommandFailed {
                operation,
                status,
                stderr,
            } => write!(
                formatter,
                "git {operation} failed (status {}): {stderr}",
                status.map_or_else(|| "unknown".to_owned(), |code| code.to_string())
            ),
            Self::OutputTooLarge(operation) => {
                write!(formatter, "git {operation} exceeded output limit")
            }
        }
    }
}

impl std::error::Error for GitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for GitError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
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
            "repository validation",
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
            "worktree create",
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

    pub fn head(&self, worktree: &Worktree) -> Result<String, GitError> {
        self.ensure_worktree(worktree)?;
        let output = run(
            Command::new("git")
                .arg("-C")
                .arg(&worktree.path)
                .args(["rev-parse", "HEAD"]),
            None,
            "read HEAD",
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
            "status",
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
            "diff",
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
            "apply patch",
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
                "worktree cleanup",
            )?;
        } else {
            run(
                Command::new("git")
                    .arg("-C")
                    .arg(&self.repository_root)
                    .args(["worktree", "prune"]),
                None,
                "worktree prune",
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
                "branch cleanup",
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
            "base commit validation",
        )?;
        text_output(&output)
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
    operation: &'static str,
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
    let (stderr, _) = join_reader(stderr_reader)?;
    command_result(status, stdout, stderr, stdout_overflow, operation)
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
        .map_err(GitError::Io)
}

fn command_result(
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_overflow: bool,
    operation: &'static str,
) -> Result<Vec<u8>, GitError> {
    if !status.success() {
        return Err(GitError::CommandFailed {
            operation,
            status: status.code(),
            stderr: sanitize(&stderr),
        });
    }
    if stdout_overflow {
        return Err(GitError::OutputTooLarge(operation));
    }
    Ok(stdout)
}

fn sanitize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
        .collect::<String>()
        .trim()
        .to_owned()
}

fn text_output(output: &[u8]) -> Result<String, GitError> {
    String::from_utf8(output.to_vec())
        .map(|value| value.trim().to_owned())
        .map_err(|_| GitError::InvalidInput("git output was not UTF-8"))
}

fn path_output(output: &[u8]) -> Result<PathBuf, GitError> {
    text_output(output).map(PathBuf::from)
}
