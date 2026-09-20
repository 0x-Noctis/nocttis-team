use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ai_team::{
    domain::{
        self,
        task::{AllowedPath, MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits},
    },
    store::{self, artifact::ArtifactStore},
};

#[path = "../src/context/mod.rs"]
mod context;

use context::{
    ContextBuilder, ContextError, ContextLimits, ContextRequest, ContextSourceKind,
    TruncationReason,
};

static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noctis-context-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn limits() -> ContextLimits {
    ContextLimits {
        max_bytes: 16_000,
        max_tokens: 4_000,
        max_file_bytes: 4_000,
        max_command_output_bytes: 4_000,
    }
}

fn contract(allowed_paths: &[&str], context_refs: &[&str]) -> TaskContract {
    TaskContract {
        id: text("id", "M2-006"),
        project_id: text("project_id", "noctis"),
        project_run_id: text("project_run_id", "run-1"),
        title: text("title", "Context builder"),
        role: text("role", "worker"),
        objective: text("objective", "Build relevant context"),
        depends_on: Vec::new(),
        allowed_paths: allowed_paths
            .iter()
            .map(|path| AllowedPath::parse(*path).unwrap())
            .collect(),
        context_refs: context_refs
            .iter()
            .map(|reference| text("context_refs", reference))
            .collect(),
        acceptance_criteria: vec![text("acceptance_criteria", "tests pass")],
        verification_commands: vec![text("verification_commands", "cargo test")],
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new("max_input_tokens", 4_000).unwrap(),
            max_output_tokens: PositiveLimit::new("max_output_tokens", 1_000).unwrap(),
            max_tool_calls: PositiveLimit::new("max_tool_calls", 10).unwrap(),
            max_attempts: MaxAttempts::new(2).unwrap(),
            timeout_seconds: PositiveLimit::new("timeout_seconds", 60).unwrap(),
        },
    }
}

fn text(field: &'static str, value: &str) -> NonEmptyString {
    NonEmptyString::parse(field, value).unwrap()
}

fn stores(repository: &TestDirectory, artifacts: &TestDirectory) -> ArtifactStore {
    let _ = repository;
    ArtifactStore::new(&artifacts.0, 8_000).unwrap()
}

fn source_refs(result: &context::BuiltContext) -> Vec<&str> {
    result
        .entries
        .iter()
        .filter(|entry| entry.source.kind == ContextSourceKind::SourceFile)
        .map(|entry| entry.source.reference.as_str())
        .collect()
}

#[test]
fn search_is_relevant_ordered_and_records_source_metadata() {
    let repository = TestDirectory::new("relevance-repo");
    let artifacts = TestDirectory::new("relevance-artifacts");
    repository.write("src/z.rs", b"needle z");
    repository.write("src/a.rs", b"needle a");
    repository.write("src/no.rs", b"irrelevant");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    let request = ContextRequest {
        search_terms: vec!["needle".into()],
        excerpts: Vec::new(),
    };

    let first = builder
        .build(&contract(&["src/**"], &[]), &request)
        .unwrap();
    let second = builder
        .build(&contract(&["src/**"], &[]), &request)
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(source_refs(&first), ["src/a.rs", "src/z.rs"]);
    let source = &first.entries[1].source;
    assert_eq!(source.reason, "matched search term \"needle\"");
    assert_eq!(source.original_bytes, source.included_bytes);
    assert_eq!(source.truncation_reason, None);
}

#[test]
fn exact_allowed_path_and_recursive_glob_have_distinct_behavior() {
    let repository = TestDirectory::new("glob-repo");
    let artifacts = TestDirectory::new("glob-artifacts");
    repository.write("src/exact.rs", b"exact");
    repository.write("src/direct.rs", b"direct");
    repository.write("src/nested/match.rs", b"nested");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    let exact = builder
        .build(
            &contract(&["src/exact.rs"], &[]),
            &ContextRequest {
                excerpts: vec!["src/exact.rs".into()],
                ..ContextRequest::default()
            },
        )
        .unwrap();
    assert_eq!(source_refs(&exact), ["src/exact.rs"]);
    assert_eq!(
        builder.build(
            &contract(&["src/exact.rs"], &[]),
            &ContextRequest {
                excerpts: vec!["src/nested/match.rs".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::PathNotAllowed)
    );
    let recursive = builder
        .build(
            &contract(&["src/**"], &[]),
            &ContextRequest {
                excerpts: vec!["src/nested/match.rs".into()],
                ..ContextRequest::default()
            },
        )
        .unwrap();
    assert_eq!(source_refs(&recursive), ["src/nested/match.rs"]);
    let direct = builder
        .build(
            &contract(&["src/*.rs"], &[]),
            &ContextRequest {
                excerpts: vec!["src/direct.rs".into()],
                ..ContextRequest::default()
            },
        )
        .unwrap();
    assert_eq!(source_refs(&direct), ["src/direct.rs"]);
    assert_eq!(
        builder.build(
            &contract(&["src/*.rs"], &[]),
            &ContextRequest {
                excerpts: vec!["src/nested/match.rs".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::PathNotAllowed)
    );
}

#[test]
fn traversal_absolute_control_and_secret_paths_are_rejected() {
    let repository = TestDirectory::new("paths-repo");
    let artifacts = TestDirectory::new("paths-artifacts");
    repository.write("safe.txt", b"safe");
    repository.write(".env", b"SECRET=value");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    let task = contract(&["**"], &[]);

    for path in [
        "../safe.txt",
        "/etc/passwd",
        "C:/secret.txt",
        "safe\\file",
        "safe\nfile",
    ] {
        assert_eq!(
            builder.build(
                &task,
                &ContextRequest {
                    excerpts: vec![path.into()],
                    ..ContextRequest::default()
                }
            ),
            Err(ContextError::InvalidPath)
        );
    }
    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                excerpts: vec![".env".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::PathNotAllowed)
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;

    let repository = TestDirectory::new("symlink-repo");
    let artifacts = TestDirectory::new("symlink-artifacts");
    let outside = TestDirectory::new("symlink-outside");
    outside.write("secret.txt", b"secret");
    symlink(outside.0.join("secret.txt"), repository.0.join("link.txt")).unwrap();
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    assert_eq!(
        builder.build(
            &contract(&["**"], &[]),
            &ContextRequest {
                excerpts: vec!["link.txt".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::Symlink)
    );
}

#[test]
fn binary_and_oversized_files_are_rejected() {
    let repository = TestDirectory::new("file-repo");
    let artifacts = TestDirectory::new("file-artifacts");
    repository.write("binary.dat", b"text\0binary");
    repository.write("large.txt", b"12345");
    let artifact_store = stores(&repository, &artifacts);
    let mut small_limits = limits();
    small_limits.max_file_bytes = 4;
    let builder = ContextBuilder::new(&repository.0, &artifact_store, small_limits).unwrap();
    let task = contract(&["**"], &[]);

    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                excerpts: vec!["binary.dat".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::FileTooLarge)
    );
    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                excerpts: vec!["large.txt".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::FileTooLarge)
    );

    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                excerpts: vec!["binary.dat".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::BinaryFile)
    );
}

#[test]
fn sparse_oversized_source_is_rejected_before_reading_contents() {
    let repository = TestDirectory::new("sparse-repo");
    let artifacts = TestDirectory::new("sparse-artifacts");
    let path = repository.0.join("huge.txt");
    fs::File::create(&path)
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    assert_eq!(
        builder.build(
            &contract(&["huge.txt"], &[]),
            &ContextRequest {
                excerpts: vec!["huge.txt".into()],
                ..ContextRequest::default()
            }
        ),
        Err(ContextError::FileTooLarge)
    );
}

#[test]
fn oversized_artifact_is_rejected_from_metadata_before_content_read() {
    let repository = TestDirectory::new("large-artifact-repo");
    let artifacts = TestDirectory::new("large-artifact-data");
    let artifact_store = ArtifactStore::new(&artifacts.0, 16_000).unwrap();
    artifact_store
        .write("large", "large.txt", "text/plain", &[b'x'; 8_000], |_| {
            Ok::<_, ()>(())
        })
        .unwrap();
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    assert_eq!(
        builder.build(
            &contract(&["src/**"], &["artifact://large"]),
            &ContextRequest::default()
        ),
        Err(ContextError::FileTooLarge)
    );
}

#[test]
fn bounded_artifact_read_rejects_large_file_even_when_metadata_claims_small_size() {
    let repository = TestDirectory::new("forged-artifact-repo");
    let artifacts = TestDirectory::new("forged-artifact-data");
    let artifact_store = ArtifactStore::new(&artifacts.0, 16_000).unwrap();
    artifact_store
        .write("forged", "forged.txt", "text/plain", &[b'x'; 8_000], |_| {
            Ok::<_, ()>(())
        })
        .unwrap();
    let metadata_path = artifacts.0.join("forged.metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["size"] = 1.into();
    fs::write(metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    assert_eq!(
        builder.build(
            &contract(&["src/**"], &["artifact://forged"]),
            &ContextRequest::default()
        ),
        Err(ContextError::FileTooLarge)
    );
}

#[test]
fn additional_secret_paths_are_rejected() {
    let repository = TestDirectory::new("secrets-repo");
    let artifacts = TestDirectory::new("secrets-artifacts");
    let paths = [
        ".env.local",
        ".netrc",
        ".npmrc",
        ".pypirc",
        "credentials/value.txt",
        "credentials.json",
        "id_rsa",
        "id_ed25519",
        "cert.pem",
        "private.key",
        "bundle.p12",
        "bundle.pfx",
    ];
    for path in paths {
        repository.write(path, b"secret");
    }
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    let task = contract(&["**"], &[]);

    for path in paths {
        assert_eq!(
            builder.build(
                &task,
                &ContextRequest {
                    excerpts: vec![path.into()],
                    ..ContextRequest::default()
                }
            ),
            Err(ContextError::PathNotAllowed),
            "{path}"
        );
    }
}

#[test]
fn request_counts_and_pattern_lengths_are_bounded() {
    let repository = TestDirectory::new("caps-repo");
    let artifacts = TestDirectory::new("caps-artifacts");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    let task = contract(&["**"], &[]);

    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                search_terms: vec!["term".into(); 33],
                excerpts: Vec::new(),
            }
        ),
        Err(ContextError::InputLimitExceeded)
    );
    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                search_terms: Vec::new(),
                excerpts: vec!["file".into(); 129],
            }
        ),
        Err(ContextError::InputLimitExceeded)
    );
    assert_eq!(
        builder.build(
            &contract(&[&"*".repeat(513)], &[]),
            &ContextRequest::default()
        ),
        Err(ContextError::InputLimitExceeded)
    );
    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                search_terms: vec!["x".repeat(257)],
                excerpts: Vec::new(),
            }
        ),
        Err(ContextError::InputLimitExceeded)
    );
    assert_eq!(
        builder.build(
            &task,
            &ContextRequest {
                search_terms: Vec::new(),
                excerpts: vec!["x".repeat(1_025)],
            }
        ),
        Err(ContextError::InputLimitExceeded)
    );
}

#[test]
fn task_contract_is_bounded_before_serialization() {
    let repository = TestDirectory::new("contract-cap-repo");
    let artifacts = TestDirectory::new("contract-cap-artifacts");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    let mut oversized_objective = contract(&["**"], &[]);
    oversized_objective.objective = text("objective", &"x".repeat(4_097));
    assert_eq!(
        builder.build(&oversized_objective, &ContextRequest::default()),
        Err(ContextError::InputLimitExceeded)
    );

    let mut oversized_list = contract(&["**"], &[]);
    oversized_list.acceptance_criteria = vec![text("acceptance_criteria", "item"); 129];
    assert_eq!(
        builder.build(&oversized_list, &ContextRequest::default()),
        Err(ContextError::InputLimitExceeded)
    );

    let mut oversized_serialized = contract(&["**"], &[]);
    oversized_serialized.acceptance_criteria =
        vec![text("acceptance_criteria", &"x".repeat(4_000)); 20];
    assert_eq!(
        builder.build(&oversized_serialized, &ContextRequest::default()),
        Err(ContextError::InputLimitExceeded)
    );
}

#[test]
fn project_run_id_obeys_contract_boundary() {
    let repository = TestDirectory::new("run-id-repo");
    let artifacts = TestDirectory::new("run-id-artifacts");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    let mut oversized = contract(&["**"], &[]);
    oversized.project_run_id = text("project_run_id", &"x".repeat(4_097));
    assert_eq!(
        builder.build(&oversized, &ContextRequest::default()),
        Err(ContextError::InputLimitExceeded)
    );

    let mut serialized = serde_json::to_value(contract(&["**"], &[])).unwrap();
    serialized["project_run_id"] = "".into();
    assert!(serde_json::from_value::<TaskContract>(serialized).is_err());
}

#[test]
fn unicode_truncation_remains_valid_utf8_and_within_budgets() {
    let repository = TestDirectory::new("unicode-repo");
    let artifacts = TestDirectory::new("unicode-artifacts");
    repository.write("unicode.txt", "éééééé".as_bytes());
    let artifact_store = stores(&repository, &artifacts);
    let task = contract(&["unicode.txt"], &[]);
    let mut small = limits();
    small.max_bytes = serde_json::to_string(&task).unwrap().len() + 5;
    let result = ContextBuilder::new(&repository.0, &artifact_store, small)
        .unwrap()
        .build(
            &task,
            &ContextRequest {
                excerpts: vec!["unicode.txt".into()],
                ..ContextRequest::default()
            },
        )
        .unwrap();

    assert!(result.bytes <= small.max_bytes);
    assert!(result.estimated_tokens <= small.max_tokens);
    assert_eq!(
        result.bytes,
        result
            .entries
            .iter()
            .map(|entry| entry.content.len())
            .sum::<usize>()
    );
    assert_eq!(
        result.estimated_tokens,
        result
            .entries
            .iter()
            .map(|entry| context::estimate_tokens(entry.content.len()))
            .sum::<usize>()
    );
    assert_eq!(result.entries.last().unwrap().content, "éé");
}

#[test]
fn byte_and_token_budgets_truncate_before_return() {
    let repository = TestDirectory::new("budget-repo");
    let artifacts = TestDirectory::new("budget-artifacts");
    repository.write("source.txt", &[b'x'; 1_000]);
    let artifact_store = stores(&repository, &artifacts);
    let task = contract(&["source.txt"], &[]);
    let request = ContextRequest {
        excerpts: vec!["source.txt".into()],
        ..ContextRequest::default()
    };

    let mut byte_limits = limits();
    byte_limits.max_bytes = 700;
    let byte_result = ContextBuilder::new(&repository.0, &artifact_store, byte_limits)
        .unwrap()
        .build(&task, &request)
        .unwrap();
    assert!(byte_result.bytes <= 700);
    assert_eq!(
        byte_result.entries.last().unwrap().source.truncation_reason,
        Some(TruncationReason::Bytes)
    );

    let mut token_limits = limits();
    token_limits.max_tokens = 180;
    let token_result = ContextBuilder::new(&repository.0, &artifact_store, token_limits)
        .unwrap()
        .build(&task, &request)
        .unwrap();
    assert!(token_result.estimated_tokens <= 180);
    assert_eq!(
        token_result
            .entries
            .last()
            .unwrap()
            .source
            .truncation_reason,
        Some(TruncationReason::Tokens)
    );
}

#[test]
fn valid_artifact_reference_is_loaded_and_invalid_reference_is_rejected() {
    let repository = TestDirectory::new("artifact-repo");
    let artifacts = TestDirectory::new("artifact-data");
    let artifact_store = stores(&repository, &artifacts);
    artifact_store
        .write(
            "decision-1",
            "decision.md",
            "text/markdown",
            b"approved",
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();

    let result = builder
        .build(
            &contract(&["src/**"], &["artifact://decision-1"]),
            &ContextRequest::default(),
        )
        .unwrap();
    assert_eq!(result.entries[1].content, "approved");
    assert_eq!(result.entries[1].source.kind, ContextSourceKind::Artifact);
    assert_eq!(
        builder.build(
            &contract(&["src/**"], &["file:///etc/passwd"]),
            &ContextRequest::default()
        ),
        Err(ContextError::InvalidArtifactReference)
    );
}

#[test]
fn rg_search_does_not_execute_shell_syntax() {
    let repository = TestDirectory::new("injection-repo");
    let artifacts = TestDirectory::new("injection-artifacts");
    repository.write("src/code.txt", b"ordinary content");
    let marker = repository.0.join("owned");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    let search = format!("missing; touch {}", marker.display());

    let result = builder
        .build(
            &contract(&["src/**"], &[]),
            &ContextRequest {
                search_terms: vec![search],
                excerpts: Vec::new(),
            },
        )
        .unwrap();
    assert_eq!(source_refs(&result), Vec::<&str>::new());
    assert!(!marker.exists());
}

#[test]
fn command_output_limit_is_recorded() {
    let repository = TestDirectory::new("command-limit-repo");
    let artifacts = TestDirectory::new("command-limit-artifacts");
    for index in 0..30 {
        repository.write(&format!("src/file-{index:02}.txt"), b"match");
    }
    let artifact_store = stores(&repository, &artifacts);
    let mut small = limits();
    small.max_command_output_bytes = 30;
    let builder = ContextBuilder::new(&repository.0, &artifact_store, small).unwrap();
    let result = builder
        .build(
            &contract(&["src/**"], &[]),
            &ContextRequest {
                search_terms: vec!["match".into()],
                excerpts: Vec::new(),
            },
        )
        .unwrap();

    assert!(
        result.entries.iter().any(|entry| {
            entry.source.truncation_reason == Some(TruncationReason::CommandOutput)
        })
    );
}

#[test]
fn errors_do_not_expose_repository_path() {
    let repository = TestDirectory::new("error-repo");
    let artifacts = TestDirectory::new("error-artifacts");
    let artifact_store = stores(&repository, &artifacts);
    let builder = ContextBuilder::new(&repository.0, &artifact_store, limits()).unwrap();
    let error = builder
        .build(
            &contract(&["**"], &[]),
            &ContextRequest {
                excerpts: vec!["missing".into()],
                ..ContextRequest::default()
            },
        )
        .unwrap_err()
        .to_string();

    assert!(!error.contains(repository.0.to_string_lossy().as_ref()));
}
