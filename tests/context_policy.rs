#[path = "../src/context/policy.rs"]
mod policy;

use policy::{ContextKind as Kind, ContextPart, PolicyError, Role, apply};

fn part(kind: Kind, content: &str) -> ContextPart {
    ContextPart {
        kind,
        content: content.into(),
    }
}

#[test]
fn roles_only_receive_required_categories() {
    let parts = [
        Kind::Task,
        Kind::RepositoryMap,
        Kind::Decision,
        Kind::SkillMetadata,
        Kind::Skill,
        Kind::Source,
        Kind::Patch,
        Kind::Check,
    ]
    .map(|kind| part(kind, "content"));
    for (role, expected, limit) in [
        (
            Role::Lead,
            vec![Kind::Task, Kind::RepositoryMap, Kind::SkillMetadata],
            12_000,
        ),
        (
            Role::Architect,
            vec![Kind::Task, Kind::RepositoryMap, Kind::Decision],
            20_000,
        ),
        (
            Role::Worker,
            vec![Kind::Task, Kind::Decision, Kind::Skill, Kind::Source],
            30_000,
        ),
        (
            Role::Reviewer,
            vec![Kind::Task, Kind::Patch, Kind::Check, Kind::Source],
            20_000,
        ),
        (Role::Verifier, vec![Kind::Task, Kind::Check], 8_000),
    ] {
        assert_eq!(role.max_input_tokens(), limit);
        let result = apply(role, parts.clone(), limit).unwrap();
        assert_eq!(
            result.parts.iter().map(|p| p.kind).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(result.snapshot.estimated_tokens, expected.len() * 2);
    }
}

#[test]
fn source_is_trimmed_after_required_artifacts_without_splitting_utf8() {
    let result = apply(
        Role::Reviewer,
        [
            part(Kind::Source, "éééééé"),
            part(Kind::Task, "task"),
            part(Kind::Patch, "diff"),
        ],
        3,
    )
    .unwrap();
    assert_eq!(result.parts[0].kind, Kind::Task);
    assert_eq!(result.parts[1].kind, Kind::Patch);
    assert_eq!(result.parts[2].content, "éé");
    assert_eq!(result.snapshot.bytes, 12);
    assert_eq!(result.snapshot.estimated_tokens, 3);
    assert!(result.snapshot.trimmed);
    assert_eq!(result.snapshot.items[2].bytes, 4);
}

#[test]
fn oversized_required_context_requests_split_and_invalid_limit_fails() {
    assert_eq!(
        apply(Role::Lead, [part(Kind::Task, "too long")], 1),
        Err(PolicyError::SplitRequired)
    );
    assert_eq!(
        apply(Role::Worker, [part(Kind::Source, "text")], 0),
        Err(PolicyError::InvalidLimit)
    );
    let result = apply(
        Role::Verifier,
        [part(Kind::Task, &"x".repeat(32_004))],
        30_000,
    );
    assert_eq!(result, Err(PolicyError::SplitRequired));
}

#[test]
fn snapshot_contains_only_category_and_size_not_content_or_references() {
    let sensitive = "test-only-sensitive-value";
    let result = apply(Role::Worker, [part(Kind::Decision, sensitive)], 100).unwrap();
    let json = serde_json::to_string(&result.snapshot).unwrap();
    assert!(!json.contains(sensitive));
    assert!(!json.contains("reference"));
    assert_eq!(result.parts[0].content, sensitive);
    assert_eq!(result.snapshot.bytes, sensitive.len());
    assert!(!result.snapshot.trimmed);
}
