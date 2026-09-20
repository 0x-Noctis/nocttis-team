use ai_team::{
    runner::{
        git::{GitWorktreeManager, Worktree},
        policy::{ToolPolicy, ToolRole},
        tools::{StructuredTools, ToolErrorCode, ToolRequest, ToolResult},
    },
    store::artifact::ArtifactStore,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
    manager: GitWorktreeManager,
    worktree: Worktree,
    artifacts: ArtifactStore,
}

impl Fixture {
    fn new(argument_limit: usize, output_limit: usize, timeout: Duration) -> Self {
        let root = std::env::temp_dir().join(format!(
            "noctis-tools-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repository = root.join("repository");
        fs::create_dir_all(repository.join("src")).unwrap();
        fs::write(repository.join("src/allowed.txt"), "needle\nbase\n").unwrap();
        fs::write(repository.join("denied.txt"), "private\n").unwrap();
        git(&repository, &["init", "--initial-branch=main"]);
        git(&repository, &["config", "user.name", "Noctis Test"]);
        git(
            &repository,
            &["config", "user.email", "noctis@example.invalid"],
        );
        git(&repository, &["add", "."]);
        git(&repository, &["commit", "-m", "initial"]);
        let base = git_output(&repository, &["rev-parse", "HEAD"]);
        let manager = GitWorktreeManager::new(&repository, root.join("worktrees")).unwrap();
        let worktree = manager.create("task", "task-tools", &base).unwrap();
        let artifacts = ArtifactStore::new(root.join("artifacts"), 1024 * 1024).unwrap();
        let fixture = Self {
            root,
            repository,
            manager,
            worktree,
            artifacts,
        };
        let _ = (argument_limit, output_limit, timeout);
        fixture
    }

    fn tools(
        &self,
        role: ToolRole,
        argument_limit: usize,
        output_limit: usize,
        timeout: Duration,
    ) -> StructuredTools<'_> {
        let policy = ToolPolicy::new(
            role,
            vec!["src/**".into()],
            argument_limit,
            output_limit,
            timeout,
        )
        .unwrap();
        StructuredTools::new(policy, &self.manager, &self.worktree, &self.artifacts)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.manager.cleanup(&self.worktree);
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn valid_calls_cover_all_structured_tools_and_audit_success() {
    let fixture = Fixture::new(16 * 1024, 16 * 1024, Duration::from_secs(2));
    let tools = fixture.tools(
        ToolRole::Worker,
        16 * 1024,
        16 * 1024,
        Duration::from_secs(2),
    );
    let listed = tools.execute(ToolRequest::ListFiles { path: "src".into() });
    assert!(
        matches!(listed.result, Ok(ToolResult::Files(ref files)) if files == &["src/allowed.txt"])
    );
    assert!(listed.audit.succeeded);
    assert!(
        matches!(tools.execute(ToolRequest::SearchCode { path: "src".into(), query: "needle".into() }).result, Ok(ToolResult::SearchMatches(ref found)) if found.len() == 1)
    );
    assert!(matches!(
        tools
            .execute(ToolRequest::ReadFile {
                path: "src/allowed.txt".into()
            })
            .result,
        Ok(ToolResult::File(_))
    ));

    let patch = b"diff --git a/src/allowed.txt b/src/allowed.txt\n--- a/src/allowed.txt\n+++ b/src/allowed.txt\n@@ -1,2 +1,2 @@\n needle\n-base\n+changed\n".to_vec();
    assert!(matches!(
        tools.execute(ToolRequest::ApplyPatch { patch }).result,
        Ok(ToolResult::PatchApplied)
    ));
    assert!(
        matches!(tools.execute(ToolRequest::GitStatus).result, Ok(ToolResult::Status(ref status)) if status.contains("src/allowed.txt"))
    );
    assert!(
        matches!(tools.execute(ToolRequest::GitDiff).result, Ok(ToolResult::Diff(ref diff)) if !diff.is_empty())
    );
    assert!(matches!(
        tools
            .execute(ToolRequest::SubmitArtifact {
                path: "src/allowed.txt".into(),
                artifact_id: "result".into(),
                logical_name: "result.txt".into(),
                media_type: "text/plain".into()
            })
            .result,
        Ok(ToolResult::Artifact(_))
    ));
    assert!(matches!(
        tools
            .execute(ToolRequest::RequestHuman {
                message: "Review needed".into()
            })
            .result,
        Ok(ToolResult::HumanRequested { .. })
    ));
    assert!(git_output(&fixture.repository, &["remote"]).is_empty());
}

#[test]
fn denied_role_and_failed_call_emit_audit() {
    let fixture = Fixture::new(1024, 1024, Duration::from_secs(1));
    let tools = fixture.tools(ToolRole::Reviewer, 1024, 1024, Duration::from_secs(1));
    let execution = tools.execute(ToolRequest::ApplyPatch {
        patch: b"secret".to_vec(),
    });
    assert_eq!(execution.result.unwrap_err().code, ToolErrorCode::Denied);
    assert!(!execution.audit.succeeded);
    assert_eq!(execution.audit.error_code, Some(ToolErrorCode::Denied));
    for role in [ToolRole::Verifier, ToolRole::Architect] {
        let tools = fixture.tools(role, 1024, 1024, Duration::from_secs(1));
        assert_eq!(
            tools
                .execute(ToolRequest::ApplyPatch { patch: Vec::new() })
                .result
                .unwrap_err()
                .code,
            ToolErrorCode::Denied
        );
    }
}

#[test]
fn traversal_symlink_absolute_option_and_artifact_escape_are_denied_without_leaks() {
    let fixture = Fixture::new(4096, 4096, Duration::from_secs(1));
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        &fixture.repository,
        fixture.worktree.path().join("src/link"),
    )
    .unwrap();
    let tools = fixture.tools(ToolRole::Worker, 4096, 4096, Duration::from_secs(1));
    for path in [
        "../denied.txt",
        "/etc/passwd",
        "-option",
        "src/link/denied.txt",
        "bad\npath",
    ] {
        let execution = tools.execute(ToolRequest::ReadFile { path: path.into() });
        let error = execution.result.unwrap_err();
        assert!(matches!(
            error.code,
            ToolErrorCode::UnsafePath | ToolErrorCode::Denied
        ));
        let display = error.to_string();
        assert!(!display.contains(path));
        assert!(!display.contains(fixture.root.to_string_lossy().as_ref()));
    }
    let artifact = tools.execute(ToolRequest::SubmitArtifact {
        path: "../denied.txt".into(),
        artifact_id: "escape".into(),
        logical_name: "escape.txt".into(),
        media_type: "text/plain".into(),
    });
    assert!(artifact.result.is_err());
}

#[test]
fn denied_path_binary_and_oversized_patch_are_rejected_before_git() {
    let fixture = Fixture::new(256, 4096, Duration::from_secs(1));
    let tools = fixture.tools(ToolRole::Worker, 256, 4096, Duration::from_secs(1));
    let denied = b"diff --git a/denied.txt b/denied.txt\n--- a/denied.txt\n+++ b/denied.txt\n@@ -1 +1 @@\n-private\n+leaked\n".to_vec();
    assert_eq!(
        tools
            .execute(ToolRequest::ApplyPatch { patch: denied })
            .result
            .unwrap_err()
            .code,
        ToolErrorCode::PatchDenied
    );
    let binary = b"diff --git a/src/x b/src/x\nGIT binary patch\nliteral 1\nA\n".to_vec();
    assert_eq!(
        tools
            .execute(ToolRequest::ApplyPatch { patch: binary })
            .result
            .unwrap_err()
            .code,
        ToolErrorCode::BinaryInput
    );
    let oversized = vec![b'x'; 257];
    assert_eq!(
        tools
            .execute(ToolRequest::ApplyPatch { patch: oversized })
            .result
            .unwrap_err()
            .code,
        ToolErrorCode::TooLarge
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.join("denied.txt")).unwrap(),
        "private\n"
    );
}

#[test]
fn oversized_output_and_timeout_are_bounded() {
    let fixture = Fixture::new(4096, 4, Duration::from_secs(1));
    let bounded = fixture.tools(ToolRole::Worker, 4096, 4, Duration::from_secs(1));
    assert_eq!(
        bounded
            .execute(ToolRequest::ReadFile {
                path: "src/allowed.txt".into()
            })
            .result
            .unwrap_err()
            .code,
        ToolErrorCode::TooLarge
    );
    let timed_out = fixture.tools(ToolRole::Worker, 4096, 4096, Duration::from_nanos(1));
    assert_eq!(
        timed_out
            .execute(ToolRequest::ListFiles { path: "src".into() })
            .result
            .unwrap_err()
            .code,
        ToolErrorCode::Timeout
    );
}

#[test]
fn sparse_read_and_oversized_search_source_are_bounded() {
    let fixture = Fixture::new(4096, 64, Duration::from_secs(1));
    let sparse = fs::File::create(fixture.worktree.path().join("src/sparse.txt")).unwrap();
    sparse.set_len(128 * 1024 * 1024).unwrap();
    fs::write(
        fixture.worktree.path().join("src/large.txt"),
        vec![b'a'; 65],
    )
    .unwrap();
    let tools = fixture.tools(ToolRole::Worker, 4096, 64, Duration::from_secs(1));
    for request in [
        ToolRequest::ReadFile {
            path: "src/sparse.txt".into(),
        },
        ToolRequest::SearchCode {
            path: "src".into(),
            query: "needle".into(),
        },
    ] {
        let execution = tools.execute(request);
        assert_eq!(execution.result.unwrap_err().code, ToolErrorCode::TooLarge);
        assert_eq!(execution.audit.error_code, Some(ToolErrorCode::TooLarge));
    }
}

#[test]
fn large_tree_stops_at_listing_limit() {
    let fixture = Fixture::new(4096, 32, Duration::from_secs(1));
    for index in 0..1000 {
        fs::write(
            fixture
                .worktree
                .path()
                .join(format!("src/file-{index:04}.txt")),
            b"x",
        )
        .unwrap();
    }
    let execution = fixture
        .tools(ToolRole::Worker, 4096, 32, Duration::from_secs(1))
        .execute(ToolRequest::ListFiles { path: "src".into() });
    assert_eq!(execution.result.unwrap_err().code, ToolErrorCode::TooLarge);
    assert_eq!(execution.audit.error_code, Some(ToolErrorCode::TooLarge));
}

#[test]
fn rename_copy_malformed_and_symlink_parent_patches_are_denied() {
    let fixture = Fixture::new(16 * 1024, 16 * 1024, Duration::from_secs(1));
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        &fixture.repository,
        fixture.worktree.path().join("src/link-parent"),
    )
    .unwrap();
    let tools = fixture.tools(
        ToolRole::Worker,
        16 * 1024,
        16 * 1024,
        Duration::from_secs(1),
    );
    let cases: &[(&[u8], ToolErrorCode)] = &[
        (b"diff --git a/src/allowed.txt b/denied.txt\nsimilarity index 100%\nrename from src/allowed.txt\nrename to denied.txt\n", ToolErrorCode::PatchDenied),
        (b"diff --git a/src/allowed.txt b/denied.txt\nsimilarity index 100%\ncopy from src/allowed.txt\ncopy to denied.txt\n", ToolErrorCode::PatchDenied),
        (b"diff --git a/src/allowed.txt b/src/new.txt\n--- a/src/allowed.txt\n", ToolErrorCode::InvalidArgument),
        (b"diff --git a/src/allowed.txt b/src/link-parent/new.txt\n--- a/src/allowed.txt\n+++ b/src/link-parent/new.txt\n@@ -1 +1 @@\n-needle\n+changed\n", ToolErrorCode::UnsafePath),
    ];
    for (patch, code) in cases {
        let execution = tools.execute(ToolRequest::ApplyPatch {
            patch: patch.to_vec(),
        });
        assert_eq!(execution.result.unwrap_err().code, *code);
        assert_eq!(execution.audit.error_code, Some(*code));
    }
}

#[test]
fn timeout_contract_is_explicitly_cooperative() {
    let policy = ToolPolicy::new(
        ToolRole::Worker,
        vec!["src/**".into()],
        1,
        1,
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(
        policy.timeout_semantics(),
        "cooperative ceiling checked between filesystem operations"
    );
}

#[test]
fn public_request_surface_has_no_raw_shell_variant() {
    let request = ToolRequest::ApplyPatch {
        patch: b"SECRET_PATCH".to_vec(),
    };
    assert_eq!(request.name().to_string(), "apply_patch");
    let debug = format!("{request:?}");
    assert!(!debug.contains("Shell"));
    assert!(!debug.contains("SECRET_PATCH"));
}

fn git(directory: &Path, arguments: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .status()
            .unwrap()
            .success()
    );
}
fn git_output(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
