// M4-006: Integrator dan cabang integrasi (src/agent/integrator.rs, src/runner/integration_git.rs),
// diuji dengan repository Git sungguhan di direktori sementara.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use ai_team::{
    agent::{
        integrator::{
            ApprovedPatch, CheckResult, ConflictKind, IntegrationCheck, Integrator,
            IntegratorError, TaskIntegration,
        },
        reviewer::{Finding, ReviewDecision, ReviewOutcome, Severity},
        verifier::{VerificationReport, VerificationResult, VerificationVerdict},
    },
    domain::task::TaskStatus,
    runner::{
        git::GitWorktreeManager,
        integration_git::{IntegrationBranch, StageOutcome, patch_files},
    },
};
use uuid::Uuid;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn lines(prefix: &str, count: usize) -> String {
    (1..=count).map(|n| format!("{prefix}{n}\n")).collect()
}

/// Repository sementara + direktori worktree; dihapus saat di-drop.
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    manager: GitWorktreeManager,
    base: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture() -> Fixture {
    let root = std::env::temp_dir().join(format!("noctis-integrator-{}", Uuid::new_v4()));
    let repo = root.join("repo");
    fs::create_dir_all(repo.join("docs")).unwrap();
    git(&root, &["init", "-q", "-b", "main", repo.to_str().unwrap()]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    fs::write(repo.join("a.txt"), lines("line ", 9)).unwrap();
    fs::write(repo.join("b.txt"), lines("b", 3)).unwrap();
    fs::write(repo.join("docs/readme.md"), "# Title\n=======\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let manager = GitWorktreeManager::new(&repo, root.join("worktrees")).unwrap();
    Fixture {
        root,
        repo,
        manager,
        base,
    }
}

impl Fixture {
    /// Patch yang dibuat worker: ubah file di worktree sendiri dari commit dasar lalu ambil `diff --binary`.
    fn patch(&self, edits: &[(&str, &str)]) -> Vec<u8> {
        let id = format!("worker-{}", Uuid::new_v4().simple());
        let worktree = self
            .manager
            .create(&id, &format!("noctis-{id}"), &self.base)
            .unwrap();
        for (path, content) in edits {
            let target = worktree.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, content).unwrap();
        }
        git(worktree.path(), &["add", "-A"]);
        let patch = self.manager.diff_binary(&worktree).unwrap();
        self.manager.cleanup(&worktree).unwrap();
        patch
    }

    fn branch(&self) -> IntegrationBranch {
        IntegrationBranch::create(
            &self.manager,
            &Uuid::new_v4().simple().to_string(),
            &self.base,
        )
        .unwrap()
    }

    fn read(&self, branch: &IntegrationBranch, file: &str) -> String {
        fs::read_to_string(branch.path().join(file)).unwrap()
    }
}

fn approved() -> ReviewOutcome {
    ReviewOutcome {
        decision: ReviewDecision::Approved,
        next_status: TaskStatus::Verify,
    }
}

fn verified() -> VerificationReport {
    VerificationReport {
        verdict: VerificationVerdict::Integrate,
        proposed_status: TaskStatus::Integrate,
        results: vec![VerificationResult {
            command: "cargo test".into(),
            exit_code: Some(0),
            duration_ms: 1,
            timed_out: false,
            stdout_artifact_id: "out".into(),
            stderr_artifact_id: "err".into(),
            passed: true,
        }],
    }
}

fn task(id: &str, deps: &[&str], patch: Vec<u8>, allowed: &[&str]) -> ApprovedPatch {
    ApprovedPatch::new(
        id,
        deps.iter().map(|dep| (*dep).to_owned()).collect(),
        patch,
        allowed.iter().map(|path| (*path).to_owned()).collect(),
        &approved(),
        &verified(),
    )
    .unwrap()
}

fn always_passes() -> impl IntegrationCheck {
    |_: &Path| CheckResult {
        passed: true,
        summary: String::new(),
    }
}

/// Gagal bila a.txt memuat "BAD": tiruan test integrasi yang menangkap tabrakan semantik.
fn rejects_bad_a() -> impl IntegrationCheck {
    |path: &Path| {
        let content = fs::read_to_string(path.join("a.txt")).unwrap_or_default();
        CheckResult {
            passed: !content.contains("BAD"),
            summary: "integration test failed: a.txt contains BAD".into(),
        }
    }
}

fn conflict_of(result: &TaskIntegration) -> &ai_team::agent::integrator::ConflictReport {
    match result {
        TaskIntegration::Conflict(report) => report,
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[test]
fn clean_patches_are_applied_in_order_and_committed_on_the_integration_branch_only() {
    let f = fixture();
    let main_before = git(&f.repo, &["rev-parse", "main"]);
    let branch = f.branch();
    let a = f.patch(&[("a.txt", &lines("line ", 9).replace("line 2", "line TWO"))]);
    let b = f.patch(&[("b.txt", "b1\nbeta\nb3\n"), ("docs/new.md", "new file\n")]);

    let report = Integrator::integrate(
        &branch,
        &[
            task("t-a", &[], a, &["a.txt"]),
            task("t-b", &["t-a"], b, &["b.txt", "docs/**"]),
        ],
        &always_passes(),
    )
    .unwrap();

    assert_eq!(
        report
            .results
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["t-a", "t-b"]
    );
    assert!(report.results.iter().all(|(_, result)| matches!(
        result,
        TaskIntegration::Integrated {
            three_way: false,
            ..
        }
    )));
    assert_eq!(f.read(&branch, "a.txt").lines().nth(1), Some("line TWO"));
    assert_eq!(f.read(&branch, "docs/new.md"), "new file\n");
    // Satu commit per task, urut, di atas commit dasar; HEAD laporan = HEAD branch.
    let log = git(
        branch.path(),
        &["log", "--format=%s", &format!("{}..HEAD", f.base)],
    );
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["integrate t-b", "integrate t-a"]
    );
    assert_eq!(
        report.head_commit,
        git(branch.path(), &["rev-parse", "HEAD"])
    );
    assert_eq!(report.base_commit, f.base);
    assert_eq!(report.results[1].1.transitions(), [TaskStatus::Done]);
    // Branch dasar tidak dimutasi sama sekali.
    assert_eq!(git(&f.repo, &["rev-parse", "main"]), main_before);
    assert_eq!(git(&f.repo, &["status", "--porcelain"]), "");
    assert_eq!(
        fs::read_to_string(f.repo.join("a.txt")).unwrap(),
        lines("line ", 9)
    );
    assert!(!f.repo.join("docs/new.md").exists());
}

#[test]
fn shifted_context_is_resolved_with_a_three_way_merge() {
    let f = fixture();
    let branch = f.branch();
    let one = f.patch(&[("a.txt", &lines("line ", 9).replace("line 3", "line THREE"))]);
    // Konteks patch kedua memuat baris 3 yang sudah diubah patch pertama -> apply langsung gagal, merge 3 arah berhasil.
    let two = f.patch(&[("a.txt", &lines("line ", 9).replace("line 5", "line FIVE"))]);

    let report = Integrator::integrate(
        &branch,
        &[
            task("one", &[], one, &["a.txt"]),
            task("two", &[], two, &["a.txt"]),
        ],
        &always_passes(),
    )
    .unwrap();

    assert!(matches!(
        report.results[0].1,
        TaskIntegration::Integrated {
            three_way: false,
            ..
        }
    ));
    assert!(
        matches!(
            report.results[1].1,
            TaskIntegration::Integrated {
                three_way: true,
                ..
            }
        ),
        "{:?}",
        report.results[1]
    );
    let merged = f.read(&branch, "a.txt");
    assert!(merged.contains("line THREE") && merged.contains("line FIVE"));
}

#[test]
fn overlapping_edits_are_a_mechanical_conflict_and_roll_back_cleanly() {
    let f = fixture();
    let branch = f.branch();
    let one = f.patch(&[(
        "a.txt",
        &lines("line ", 9).replace("line 5", "line FIVE-ONE"),
    )]);
    let two = f.patch(&[(
        "a.txt",
        &lines("line ", 9).replace("line 5", "line FIVE-TWO"),
    )]);
    let three = f.patch(&[("b.txt", "b1\nb2\nb3-changed\n")]);

    let report = Integrator::integrate(
        &branch,
        &[
            task("one", &[], one, &["a.txt"]),
            task("two", &[], two, &["a.txt"]),
            task("three", &[], three, &["b.txt"]),
        ],
        &always_passes(),
    )
    .unwrap();

    let conflict = conflict_of(&report.results[1].1);
    assert_eq!(
        (
            conflict.task_id.as_str(),
            conflict.kind,
            conflict.files.clone()
        ),
        ("two", ConflictKind::Mechanical, vec!["a.txt".to_owned()])
    );
    assert!(
        !conflict.detail.contains(branch.path().to_str().unwrap()),
        "path absolut tidak boleh bocor ke laporan"
    );
    assert_eq!(
        report.results[1].1.transitions(),
        [TaskStatus::Conflict, TaskStatus::NeedsHuman]
    );
    // Rollback: a.txt tetap hasil patch pertama, tanpa penanda konflik; task independen tetap terintegrasi.
    assert!(
        f.read(&branch, "a.txt").contains("line FIVE-ONE")
            && !f.read(&branch, "a.txt").contains("<<<<<<<")
    );
    assert!(matches!(
        report.results[2].1,
        TaskIntegration::Integrated { .. }
    ));
    assert!(branch.is_clean().unwrap());
}

#[test]
fn leftover_conflict_markers_are_caught_and_rolled_back() {
    let f = fixture();
    let branch = f.branch();
    let head_before = branch.head().unwrap();
    let marked = f.patch(&[(
        "b.txt",
        "b1\n<<<<<<< ours\nx\n=======\ny\n>>>>>>> theirs\nb3\n",
    )]);
    // Garis "=======" saja sah di Markdown dan tidak dianggap penanda.
    let markdown = f.patch(&[("docs/readme.md", "# Title\n=======\nmore text\n")]);

    let report = Integrator::integrate(
        &branch,
        &[
            task("marked", &[], marked, &["b.txt"]),
            task("md", &[], markdown, &["docs/**"]),
        ],
        &always_passes(),
    )
    .unwrap();

    let conflict = conflict_of(&report.results[0].1);
    assert_eq!(
        (conflict.kind, conflict.files.clone()),
        (ConflictKind::ConflictMarkers, vec!["b.txt".to_owned()])
    );
    assert_eq!(
        f.read(&branch, "b.txt"),
        lines("b", 3),
        "patch bermarker tidak tertinggal"
    );
    assert!(matches!(
        report.results[1].1,
        TaskIntegration::Integrated { .. }
    ));
    assert_ne!(branch.head().unwrap(), head_before);
}

#[test]
fn failed_integration_check_rolls_the_patch_back_and_blocks_dependents() {
    let f = fixture();
    let branch = f.branch();
    let good = f.patch(&[("b.txt", "b1\nb2\nb3-ok\n")]);
    let bad = f.patch(&[("a.txt", &lines("line ", 9).replace("line 7", "line BAD"))]);
    let dependent = f.patch(&[("docs/readme.md", "# Title\n=======\ndepends on bad\n")]);
    let independent = f.patch(&[("docs/extra.md", "extra\n")]);

    let report = Integrator::integrate(
        &branch,
        &[
            task("good", &[], good, &["b.txt"]),
            task("bad", &[], bad, &["a.txt"]),
            task("dependent", &["bad"], dependent, &["docs/**"]),
            task("independent", &["good"], independent, &["docs/**"]),
        ],
        &rejects_bad_a(),
    )
    .unwrap();

    let regression = conflict_of(&report.results[1].1);
    assert_eq!(regression.kind, ConflictKind::RegressionFailed);
    assert!(regression.detail.contains("a.txt contains BAD"));
    assert_eq!(
        report.results[2].1,
        TaskIntegration::Skipped {
            blocked_by: "bad".into()
        }
    );
    assert!(
        report.results[2].1.transitions().is_empty(),
        "task yang dilewati tetap INTEGRATE"
    );
    assert!(matches!(
        report.results[3].1,
        TaskIntegration::Integrated { .. }
    ));
    // Patch yang gagal tidak ada di branch; yang lain tetap.
    assert!(!f.read(&branch, "a.txt").contains("BAD"));
    assert!(f.read(&branch, "b.txt").contains("b3-ok"));
    assert!(branch.is_clean().unwrap());
}

#[test]
fn patches_outside_the_allowed_paths_are_rejected_without_touching_git() {
    let f = fixture();
    let branch = f.branch();
    let head_before = branch.head().unwrap();
    let sneaky = f.patch(&[
        ("a.txt", &lines("line ", 9).replace("line 1", "line ONE")),
        ("b.txt", "b1\nhijacked\nb3\n"),
    ]);

    let report = Integrator::integrate(
        &branch,
        &[task("sneaky", &[], sneaky, &["a.txt"])],
        &always_passes(),
    )
    .unwrap();

    let conflict = conflict_of(&report.results[0].1);
    assert_eq!(
        (conflict.kind, conflict.files.clone()),
        (ConflictKind::ScopeViolation, vec!["b.txt".to_owned()])
    );
    assert_eq!(branch.head().unwrap(), head_before);
    assert!(branch.is_clean().unwrap());
    assert_eq!(f.read(&branch, "b.txt"), lines("b", 3));
}

#[test]
fn only_approved_and_verified_tasks_can_become_patches() {
    let f = fixture();
    let patch = f.patch(&[("b.txt", "b1\nx\nb3\n")]);
    let build = |review: &ReviewOutcome, verification: &VerificationReport| {
        ApprovedPatch::new(
            "t",
            vec![],
            patch.clone(),
            vec!["b.txt".into()],
            review,
            verification,
        )
    };
    assert!(build(&approved(), &verified()).is_ok());

    let changes = ReviewOutcome {
        decision: ReviewDecision::ChangesRequested {
            findings: vec![Finding {
                severity: Severity::High,
                code: "missing_requirement".into(),
                message: "x".into(),
                path: None,
                line: None,
            }],
        },
        next_status: TaskStatus::ChangesRequested,
    };
    assert!(matches!(
        build(&changes, &verified()),
        Err(IntegratorError::NotApprovedAndVerified)
    ));
    let wrong_next = ReviewOutcome {
        decision: ReviewDecision::Approved,
        next_status: TaskStatus::Review,
    };
    assert!(matches!(
        build(&wrong_next, &verified()),
        Err(IntegratorError::NotApprovedAndVerified)
    ));

    let mut failed = verified();
    failed.verdict = VerificationVerdict::Failed;
    failed.proposed_status = TaskStatus::Failed;
    assert!(matches!(
        build(&approved(), &failed),
        Err(IntegratorError::NotApprovedAndVerified)
    ));
    let mut empty = verified();
    empty.results.clear();
    assert!(matches!(
        build(&approved(), &empty),
        Err(IntegratorError::NotApprovedAndVerified)
    ));
    let mut one_failed = verified();
    one_failed.results[0].passed = false;
    assert!(matches!(
        build(&approved(), &one_failed),
        Err(IntegratorError::NotApprovedAndVerified)
    ));
    let mut timed_out = verified();
    timed_out.results[0].timed_out = true;
    assert!(matches!(
        build(&approved(), &timed_out),
        Err(IntegratorError::NotApprovedAndVerified)
    ));

    assert!(matches!(
        ApprovedPatch::new(
            "t",
            vec![],
            vec![],
            vec!["b.txt".into()],
            &approved(),
            &verified()
        ),
        Err(IntegratorError::InvalidPatch(_))
    ));
    assert!(matches!(
        ApprovedPatch::new("t", vec![], patch.clone(), vec![], &approved(), &verified()),
        Err(IntegratorError::InvalidPatch(_))
    ));
    assert!(matches!(
        ApprovedPatch::new(
            " ",
            vec![],
            patch,
            vec!["b.txt".into()],
            &approved(),
            &verified()
        ),
        Err(IntegratorError::InvalidPatch(_))
    ));
}

#[test]
fn rollback_returns_the_branch_to_an_earlier_commit() {
    let f = fixture();
    let branch = f.branch();
    let one = f.patch(&[("b.txt", "b1\nb2-one\nb3\n")]);
    let two = f.patch(&[("a.txt", &lines("line ", 9).replace("line 9", "line NINE"))]);
    let report = Integrator::integrate(
        &branch,
        &[
            task("one", &[], one, &["b.txt"]),
            task("two", &[], two, &["a.txt"]),
        ],
        &always_passes(),
    )
    .unwrap();
    let TaskIntegration::Integrated { commit: first, .. } = report.results[0].1.clone() else {
        panic!("one integrated")
    };

    // Gerbang akhir gagal -> kembali ke commit pertama; perubahan task kedua hilang, file liar dibersihkan.
    fs::write(branch.path().join("stray.txt"), "x").unwrap();
    branch.rollback_to(&first).unwrap();
    assert_eq!(branch.head().unwrap(), first);
    assert!(branch.is_clean().unwrap());
    assert!(f.read(&branch, "b.txt").contains("b2-one"));
    assert_eq!(f.read(&branch, "a.txt"), lines("line ", 9));
    branch.rollback_to(&f.base).unwrap();
    assert_eq!(f.read(&branch, "b.txt"), lines("b", 3));
    // Hanya leluhur HEAD yang valid.
    assert!(branch.rollback_to("deadbeef").is_err());
    assert!(branch.rollback_to("not-a-commit").is_err());
}

#[test]
fn nothing_is_pushed_and_the_base_branch_stays_put() {
    let f = fixture();
    let remote = f.root.join("remote.git");
    git(&f.root, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(
        &f.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let branch = f.branch();
    let patch = f.patch(&[("b.txt", "b1\nb2\nb3-pushed?\n")]);
    Integrator::integrate(
        &branch,
        &[task("t", &[], patch, &["b.txt"])],
        &always_passes(),
    )
    .unwrap();

    assert_eq!(
        git(&remote, &["for-each-ref"]),
        "",
        "remote tidak menerima ref apa pun"
    );
    assert!(git(&f.repo, &["branch", "--list", "noctis-integration-*"]).contains(branch.branch()));
    assert_eq!(git(&f.repo, &["rev-parse", "main"]), f.base);
}

#[test]
fn dirty_branches_and_duplicate_tasks_are_refused() {
    let f = fixture();
    let branch = f.branch();
    let patch = f.patch(&[("b.txt", "b1\nx\nb3\n")]);
    fs::write(branch.path().join("stray.txt"), "left over").unwrap();
    assert!(matches!(
        Integrator::integrate(
            &branch,
            &[task("t", &[], patch.clone(), &["b.txt"])],
            &always_passes()
        ),
        Err(IntegratorError::DirtyBranch)
    ));
    assert!(branch.stage_patch(&patch).is_err());
    branch.discard().unwrap();
    assert!(branch.is_clean().unwrap());

    let twice = [
        task("t", &[], patch.clone(), &["b.txt"]),
        task("t", &[], patch, &["b.txt"]),
    ];
    assert!(matches!(
        Integrator::integrate(&branch, &twice, &always_passes()),
        Err(IntegratorError::DuplicateTask)
    ));
}

#[test]
fn unreadable_or_dangerous_patches_become_conflicts_not_crashes() {
    let f = fixture();
    let branch = f.branch();
    let head_before = branch.head().unwrap();
    for (name, patch) in [
        ("garbage", b"this is not a patch\n".to_vec()),
        ("traversal", b"diff --git a/../outside b/../outside\n--- a/../outside\n+++ b/../outside\n@@ -1 +1 @@\n-x\n+y\n".to_vec()),
        ("git-dir", b"diff --git a/.git/config b/.git/config\n--- a/.git/config\n+++ b/.git/config\n@@ -1 +1 @@\n-x\n+y\n".to_vec()),
        ("absolute", b"diff --git a//etc/passwd b//etc/passwd\n".to_vec()),
        ("quoted", b"diff --git \"a/we\\\"ird\" \"b/we\\\"ird\"\n".to_vec()),
        ("rename", b"diff --git a/a.txt b/renamed.txt\nsimilarity index 100%\nrename from a.txt\nrename to renamed.txt\n".to_vec()),
    ] {
        let report = Integrator::integrate(&branch, &[task(name, &[], patch, &["**"])], &always_passes()).unwrap();
        assert_eq!(conflict_of(&report.results[0].1).kind, ConflictKind::InvalidPatch, "{name}");
    }
    assert_eq!(branch.head().unwrap(), head_before);
    assert!(branch.is_clean().unwrap());
}

#[test]
fn patch_files_lists_every_touched_path() {
    let f = fixture();
    let patch = f.patch(&[
        ("a.txt", "changed\n"),
        ("docs/readme.md", "# changed\n"),
        ("deep/dir/new.rs", "fn main() {}\n"),
    ]);
    let files: Vec<String> = patch_files(&patch).unwrap().into_iter().collect();
    assert_eq!(files, ["a.txt", "deep/dir/new.rs", "docs/readme.md"]);
    assert!(patch_files(b"").is_err());
    // Patch yang valid diterapkan oleh stage_patch dan commit-nya menjadi tepat satu commit baru.
    let branch = f.branch();
    assert!(matches!(
        branch.stage_patch(&patch).unwrap(),
        StageOutcome::Staged {
            three_way: false,
            ..
        }
    ));
    let commit = branch.commit("manual").unwrap();
    assert_eq!(
        git(
            branch.path(),
            &["rev-list", "--count", &format!("{}..{commit}", f.base)]
        ),
        "1"
    );
}
