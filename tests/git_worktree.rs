use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use ai_team::runner::git::{GitError, GitWorktreeManager, IntegrationResult};

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
    worktrees: PathBuf,
    base: String,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "noctis-git-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repository = root.join("repository");
        let worktrees = root.join("worktrees");
        fs::create_dir_all(&repository).unwrap();
        git(&repository, &["init", "--initial-branch=main"]);
        git(&repository, &["config", "user.name", "Noctis Test"]);
        git(
            &repository,
            &["config", "user.email", "noctis@example.invalid"],
        );
        fs::write(repository.join("tracked.txt"), "base\n").unwrap();
        git(&repository, &["add", "tracked.txt"]);
        git(&repository, &["commit", "-m", "initial"]);
        let base = git_output(&repository, &["rev-parse", "HEAD"]);
        Self {
            root,
            repository,
            worktrees,
            base,
        }
    }

    fn manager(&self) -> GitWorktreeManager {
        GitWorktreeManager::new(&self.repository, &self.worktrees).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn create_status_binary_diff_and_base_are_exact() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let worktree = manager
        .create("task-1", "task-1-branch", &fixture.base)
        .unwrap();

    assert_eq!(
        manager.repository_root(),
        fs::canonicalize(&fixture.repository).unwrap()
    );
    assert_eq!(
        manager.worktree_root(),
        fs::canonicalize(&fixture.worktrees).unwrap()
    );
    assert_eq!(manager.head(&worktree).unwrap(), fixture.base);
    assert_eq!(worktree.base_commit(), fixture.base);
    assert_eq!(worktree.branch(), "task-1-branch");

    fs::write(worktree.path().join("tracked.txt"), "changed\n").unwrap();
    fs::write(worktree.path().join("binary.bin"), [0, 1, 2, 0, 255]).unwrap();
    git(worktree.path(), &["add", "binary.bin"]);
    let status = manager.status(&worktree).unwrap();
    assert!(status.contains("binary.bin"));
    assert!(status.contains("tracked.txt"));
    let diff = manager.diff_binary(&worktree).unwrap();
    let diff = String::from_utf8_lossy(&diff);
    assert!(diff.contains("GIT binary patch"));
    assert!(diff.contains("tracked.txt"));
}

#[test]
fn integrates_verified_patch_into_clean_target() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let source = manager
        .create("source", "source-branch", &fixture.base)
        .unwrap();
    let target = manager
        .create("target", "target-branch", &fixture.base)
        .unwrap();
    fs::write(source.path().join("tracked.txt"), "integrated\n").unwrap();

    assert_eq!(
        manager.integrate_verified(&source, &target).unwrap(),
        IntegrationResult::Integrated
    );
    assert_eq!(
        fs::read_to_string(target.path().join("tracked.txt")).unwrap(),
        "integrated\n"
    );
    assert_eq!(manager.head(&target).unwrap(), fixture.base);
}

#[test]
fn integrates_quoted_patch_path() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let source = manager
        .create("quoted-source", "quoted-source-branch", &fixture.base)
        .unwrap();
    let target = manager
        .create("quoted-target", "quoted-target-branch", &fixture.base)
        .unwrap();
    fs::write(source.path().join("quoted path.txt"), "quoted\n").unwrap();
    git(source.path(), &["add", "quoted path.txt"]);

    assert_eq!(
        manager.integrate_verified(&source, &target).unwrap(),
        IntegrationResult::Integrated
    );
    assert_eq!(
        fs::read_to_string(target.path().join("quoted path.txt")).unwrap(),
        "quoted\n"
    );
}

#[test]
fn conflict_does_not_mutate_target() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let source = manager
        .create("source", "source-branch", &fixture.base)
        .unwrap();
    let target = manager
        .create("target", "target-branch", &fixture.base)
        .unwrap();
    fs::write(source.path().join("tracked.txt"), "source\n").unwrap();
    fs::write(target.path().join("tracked.txt"), "target\n").unwrap();
    git(target.path(), &["add", "tracked.txt"]);
    git(target.path(), &["commit", "-m", "target change"]);
    let before = fs::read(target.path().join("tracked.txt")).unwrap();
    let head = manager.head(&target).unwrap();

    assert_eq!(
        manager.integrate_verified(&source, &target).unwrap(),
        IntegrationResult::Conflict
    );
    assert_eq!(fs::read(target.path().join("tracked.txt")).unwrap(), before);
    assert_eq!(manager.head(&target).unwrap(), head);
    assert!(manager.status(&target).unwrap().is_empty());
}

#[test]
fn rejects_dirty_target_and_repository_or_base_mismatch() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let source = manager
        .create("source", "source-branch", &fixture.base)
        .unwrap();
    let target = manager
        .create("target", "target-branch", &fixture.base)
        .unwrap();
    fs::write(source.path().join("tracked.txt"), "source\n").unwrap();
    fs::write(target.path().join("tracked.txt"), "dirty\n").unwrap();
    assert!(matches!(
        manager.integrate_verified(&source, &target),
        Err(GitError::InvalidInput("target worktree is dirty"))
    ));

    fs::write(target.path().join("tracked.txt"), "base\n").unwrap();
    let other = Fixture::new();
    let foreign = other
        .manager()
        .create("foreign", "foreign-branch", &other.base)
        .unwrap();
    assert!(matches!(
        manager.integrate_verified(&foreign, &target),
        Err(GitError::InvalidInput("worktree escaped configured root"))
    ));

    git(source.path(), &["checkout", "--orphan", "unrelated"]);
    fs::write(source.path().join("unrelated.txt"), "unrelated\n").unwrap();
    git(source.path(), &["add", "unrelated.txt"]);
    git(source.path(), &["commit", "-m", "unrelated"]);
    assert!(matches!(
        manager.integrate_verified(&source, &target),
        Err(GitError::CommandFailed {
            operation: ai_team::runner::git::GitOperation::ValidateAncestry,
            ..
        })
    ));
}

#[cfg(unix)]
/// Regresi (M5-009): file baru dari patch harus ikut `diff_binary`; sebelumnya diff kosong sehingga task "selesai"
/// tanpa membawa file itu ke integrasi.
#[test]
fn files_created_by_a_patch_are_part_of_the_integration_diff() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let worktree = manager
        .create("new-file", "new-file-branch", &fixture.base)
        .unwrap();
    let patch = b"diff --git a/docs/new.md b/docs/new.md\nnew file mode 100644\n--- /dev/null\n+++ b/docs/new.md\n@@ -0,0 +1 @@\n+hello\n\
diff --git a/tracked.txt b/tracked.txt\n--- a/tracked.txt\n+++ b/tracked.txt\n@@ -1 +1 @@\n-base\n+changed\n";
    manager.apply_patch(&worktree, patch).unwrap();
    let diff = String::from_utf8(manager.diff_binary(&worktree).unwrap()).unwrap();
    assert!(
        diff.contains("docs/new.md") && diff.contains("+hello"),
        "{diff}"
    );
    assert!(diff.contains("tracked.txt"), "{diff}");
    // Diff itu dapat diterapkan ke worktree lain dan menghasilkan isi yang sama.
    let target = manager
        .create("new-file-target", "new-file-target-branch", &fixture.base)
        .unwrap();
    manager.apply_patch(&target, diff.as_bytes()).unwrap();
    assert_eq!(
        fs::read_to_string(target.path().join("docs/new.md")).unwrap(),
        "hello\n"
    );
    assert_eq!(
        fs::read_to_string(target.path().join("tracked.txt")).unwrap(),
        "changed\n"
    );
}

#[test]
fn rejects_symlink_patch_escape_without_mutating_target() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let source = manager
        .create("link-source", "link-source-branch", &fixture.base)
        .unwrap();
    let target = manager
        .create("link-target", "link-target-branch", &fixture.base)
        .unwrap();
    std::os::unix::fs::symlink("../../outside", source.path().join("escape-link")).unwrap();
    git(source.path(), &["add", "escape-link"]);

    assert!(matches!(
        manager.integrate_verified(&source, &target),
        Err(GitError::InvalidInput("patch path is symbolic link"))
    ));
    assert!(!target.path().join("escape-link").exists());
    assert!(manager.status(&target).unwrap().is_empty());
}

#[test]
fn integrates_binary_patch_and_rejects_oversized_patch() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let source = manager
        .create("binary-source", "binary-source-branch", &fixture.base)
        .unwrap();
    let target = manager
        .create("binary-target", "binary-target-branch", &fixture.base)
        .unwrap();
    let binary = [0, 255, 1, 254, 2, 0, 253];
    fs::write(source.path().join("binary.bin"), binary).unwrap();
    git(source.path(), &["add", "binary.bin"]);
    assert_eq!(
        manager.integrate_verified(&source, &target).unwrap(),
        IntegrationResult::Integrated
    );
    assert_eq!(fs::read(target.path().join("binary.bin")).unwrap(), binary);

    let large_source = manager
        .create("large-source", "large-source-branch", &fixture.base)
        .unwrap();
    let large_target = manager
        .create("large-target", "large-target-branch", &fixture.base)
        .unwrap();
    fs::write(
        large_source.path().join("tracked.txt"),
        "NOCTIS_PRIVATE_PATCH_MARKER\n".repeat(50_000),
    )
    .unwrap();
    let error = manager
        .integrate_verified(&large_source, &large_target)
        .unwrap_err();
    assert!(matches!(
        error,
        GitError::OutputTooLarge {
            operation: ai_team::runner::git::GitOperation::ReadDiff,
            ..
        }
    ));
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains("NOCTIS_PRIVATE_PATCH_MARKER"));
    assert!(!rendered.contains(fixture.root.to_string_lossy().as_ref()));
    assert!(manager.status(&large_target).unwrap().is_empty());
}

#[test]
fn reopens_dirty_retained_worktree_without_changing_it() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let created = manager
        .create("retained", "retained-branch", &fixture.base)
        .unwrap();
    fs::write(created.path().join("tracked.txt"), "dirty\n").unwrap();
    fs::write(created.path().join("untracked.txt"), "retained\n").unwrap();
    let head_before = git_output(created.path(), &["rev-parse", "HEAD"]);

    let reopened = manager
        .open("retained", "retained-branch", &fixture.base)
        .unwrap();

    assert_eq!(reopened.path(), created.path());
    assert_eq!(reopened.branch(), "retained-branch");
    assert_eq!(reopened.base_commit(), fixture.base);
    assert_eq!(
        git_output(reopened.path(), &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(
        fs::read_to_string(reopened.path().join("tracked.txt")).unwrap(),
        "dirty\n"
    );
    assert_eq!(
        fs::read_to_string(reopened.path().join("untracked.txt")).unwrap(),
        "retained\n"
    );
}

#[test]
fn reopen_rejects_branch_and_base_mismatch() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .create("retained", "retained-branch", &fixture.base)
        .unwrap();

    assert!(matches!(
        manager.open("retained", "other-branch", &fixture.base),
        Err(GitError::InvalidInput("worktree branch does not match"))
    ));

    fs::write(fixture.repository.join("later.txt"), "later\n").unwrap();
    git(&fixture.repository, &["add", "later.txt"]);
    git(&fixture.repository, &["commit", "-m", "later"]);
    let later = git_output(&fixture.repository, &["rev-parse", "HEAD"]);
    assert!(matches!(
        manager.open("retained", "retained-branch", &later),
        Err(GitError::InvalidInput("base commit is not an ancestor"))
    ));
}

#[test]
fn reopen_rejects_missing_unregistered_and_escaped_paths() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    assert!(matches!(
        manager.open("missing", "safe-branch", &fixture.base),
        Err(GitError::InvalidInput("worktree does not exist"))
    ));

    fs::create_dir_all(fixture.worktrees.join("unregistered")).unwrap();
    assert!(matches!(
        manager.open("unregistered", "safe-branch", &fixture.base),
        Err(GitError::InvalidInput("worktree is not registered"))
    ));

    #[cfg(unix)]
    {
        let outside = fixture.root.join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.worktrees.join("escaped")).unwrap();
        assert!(matches!(
            manager.open("escaped", "safe-branch", &fixture.base),
            Err(GitError::InvalidInput("worktree escaped configured root"))
        ));
    }
}

#[test]
fn reopen_errors_redact_paths_branch_and_input_markers() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .create("private-task", "private-branch-marker", &fixture.base)
        .unwrap();
    let error = manager
        .open(
            "private-task",
            "NOCTIS_PRIVATE_BRANCH_MARKER",
            &fixture.base,
        )
        .unwrap_err();
    let display = error.to_string();
    let debug = format!("{error:?}");

    for private in [
        fixture.repository.to_string_lossy().as_ref(),
        fixture.worktrees.to_string_lossy().as_ref(),
        "private-branch-marker",
        "NOCTIS_PRIVATE_BRANCH_MARKER",
    ] {
        assert!(!display.contains(private), "Display leaked private input");
        assert!(!debug.contains(private), "Debug leaked private input");
    }
}

#[test]
fn valid_patch_applies_and_invalid_patch_leaves_base_branch_unchanged() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let worktree = manager
        .create("task-2", "task-2-branch", &fixture.base)
        .unwrap();
    let patch = b"diff --git a/tracked.txt b/tracked.txt\n--- a/tracked.txt\n+++ b/tracked.txt\n@@ -1 +1 @@\n-base\n+patched\n";
    manager.apply_patch(&worktree, patch).unwrap();
    assert_eq!(
        fs::read_to_string(worktree.path().join("tracked.txt")).unwrap(),
        "patched\n"
    );

    let invalid = b"diff --git a/tracked.txt b/tracked.txt\n--- a/tracked.txt\n+++ b/tracked.txt\n@@ -9 +9 @@\n-nope\n+broken\n";
    assert!(matches!(
        manager.apply_patch(&worktree, invalid),
        Err(GitError::CommandFailed { .. })
    ));
    assert_eq!(
        fs::read_to_string(fixture.repository.join("tracked.txt")).unwrap(),
        "base\n"
    );
    assert_eq!(
        git_output(&fixture.repository, &["rev-parse", "main"]),
        fixture.base
    );
}

#[test]
fn command_errors_do_not_expose_paths_or_input_markers() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let worktree = manager
        .create("private-task", "private-branch", &fixture.base)
        .unwrap();
    let secret = "NOCTIS_SECRET_PATCH_MARKER";
    let patch = format!(
        "diff --git a/{secret}.txt b/{secret}.txt\n--- a/{secret}.txt\n+++ b/{secret}.txt\n@@ -1 +1 @@\n-{secret}\n+changed\n"
    );
    let error = manager
        .apply_patch(&worktree, patch.as_bytes())
        .unwrap_err();
    let display = error.to_string();
    let debug = format!("{error:?}");

    for private in [
        fixture.repository.to_string_lossy().as_ref(),
        worktree.path().to_string_lossy().as_ref(),
        secret,
        worktree.branch(),
    ] {
        assert!(!display.contains(private), "Display leaked {private}");
        assert!(!debug.contains(private), "Debug leaked {private}");
    }
}

#[test]
fn rejects_invalid_repository_and_malicious_components() {
    let fixture = Fixture::new();
    let invalid = fixture.root.join("not-a-repository");
    fs::create_dir(&invalid).unwrap();
    assert!(GitWorktreeManager::new(&invalid, fixture.root.join("other-worktrees")).is_err());

    let manager = fixture.manager();
    for malicious in [
        "../escape",
        "/absolute",
        "with/slash",
        "with\\slash",
        "-option",
        "bad\nname",
        "..",
    ] {
        assert!(matches!(
            manager.create(malicious, "safe-branch", &fixture.base),
            Err(GitError::InvalidInput(_))
        ));
        assert!(matches!(
            manager.create("safe-task", malicious, &fixture.base),
            Err(GitError::InvalidInput(_))
        ));
    }
    assert!(matches!(
        manager.create("safe-task", "safe-branch", "-option"),
        Err(GitError::InvalidInput(_))
    ));
}

#[test]
fn cleanup_is_idempotent_and_never_configures_a_remote() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let worktree = manager
        .create("task-3", "task-3-branch", &fixture.base)
        .unwrap();
    manager.cleanup(&worktree).unwrap();
    manager.cleanup(&worktree).unwrap();

    assert!(!worktree.path().exists());
    assert!(git_output(&fixture.repository, &["remote"]).is_empty());
    assert!(
        !Command::new("git")
            .arg("-C")
            .arg(&fixture.repository)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/task-3-branch"
            ])
            .status()
            .unwrap()
            .success()
    );
}

fn git(directory: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .status()
        .unwrap();
    assert!(status.success(), "git {arguments:?} failed");
}

fn git_output(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
