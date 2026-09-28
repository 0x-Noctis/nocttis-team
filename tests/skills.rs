use ai_team::context::policy;
#[path = "../src/context/skills.rs"]
mod skills;

use std::{fs, path::PathBuf};

use policy::Role;
use skills::{SkillError, SkillRegistry};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("noctis-skills-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn add(&self, filename: &str, id: &str, role: &str, content: &str) {
        fs::write(
            self.0.join(filename),
            format!("---\nid: {id}\nroles: [{role}]\ntriggers: [rust, axum]\nsummary: Safe review instructions.\nestimated_tokens: 8\nversion: 1\n---\n{content}"),
        ).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn parses_metadata_without_loading_content_and_rejects_bad_ids() {
    let fixture = Fixture::new();
    fixture.add(
        "safe.md",
        "rust-review",
        "backend_engineer",
        "private instruction",
    );
    let registry = SkillRegistry::new(&fixture.0).unwrap();
    let metadata = registry.metadata();
    assert_eq!(metadata[0].id, "rust-review");
    assert_eq!(metadata[0].version, 1);
    assert!(
        !serde_json::to_string(&metadata)
            .unwrap()
            .contains("private instruction")
    );
    assert_eq!(
        registry.load(
            Role::Lead,
            "rust-review",
            "backend_engineer",
            &["rust"],
            100
        ),
        Err(SkillError::Forbidden)
    );
    fixture.add("bad.md", "../escape", "backend_engineer", "content");
    assert!(matches!(
        SkillRegistry::new(&fixture.0),
        Err(SkillError::InvalidId)
    ));
    fs::remove_file(fixture.0.join("bad.md")).unwrap();
    for header in [
        "---\nid: valid\nroles: [worker]\ntriggers: [rust]\nsummary: safe\nestimated_tokens: 8\nversion: 1\nversion: 2\n---\n",
        "---\nid: valid\nroles: []\ntriggers: [rust]\nsummary: safe\nestimated_tokens: 8\nversion: 1\n---\n",
        "---\nid: valid\nroles: [worker]\ntriggers: [rust]\nsummary: safe\nestimated_tokens: 0\nversion: 1\n---\n",
        "---\nid: valid\nroles: [worker]\ntriggers: [rust]\nsummary: safe\nestimated_tokens: 8\nversion: 1\n",
    ] {
        fs::write(fixture.0.join("bad.md"), header).unwrap();
        assert!(matches!(
            SkillRegistry::new(&fixture.0),
            Err(SkillError::InvalidMetadata)
        ));
    }
    fs::remove_file(fixture.0.join("bad.md")).unwrap();
    fixture.add("duplicate.md", "rust-review", "backend_engineer", "content");
    assert!(matches!(
        SkillRegistry::new(&fixture.0),
        Err(SkillError::DuplicateId)
    ));
}

#[test]
fn worker_matches_by_role_trigger_budget_and_two_skill_limit() {
    let fixture = Fixture::new();
    for (id, role) in [
        ("a", "backend_engineer"),
        ("b", "backend_engineer"),
        ("c", "backend_engineer"),
        ("d", "reviewer"),
    ] {
        fixture.add(&format!("{id}.md"), id, role, "content");
    }
    let registry = SkillRegistry::new(&fixture.0).unwrap();
    assert!(
        registry
            .match_worker("backend_engineer", &["go"], 100)
            .is_empty()
    );
    assert_eq!(
        registry
            .match_worker("backend_engineer", &["RUST"], 15)
            .len(),
        1
    );
    let selected = registry.match_worker("backend_engineer", &["rust"], 100);
    assert_eq!(
        selected.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(
        registry
            .load(Role::Worker, "a", "backend_engineer", &["rust"], 100)
            .unwrap(),
        "content"
    );
    assert_eq!(
        registry.load(Role::Worker, "c", "backend_engineer", &["rust"], 100),
        Err(SkillError::BudgetExceeded)
    );
    assert_eq!(
        registry.load(Role::Worker, "a", "reviewer", &["rust"], 100),
        Err(SkillError::Forbidden)
    );
    assert_eq!(
        registry.load(Role::Worker, "a", "backend_engineer", &["go"], 100),
        Err(SkillError::Forbidden)
    );
    assert_eq!(
        registry.load(Role::Worker, "a", "backend_engineer", &["rust"], 1),
        Err(SkillError::BudgetExceeded)
    );
    fixture.add("a.md", "a", "backend_engineer", &"x".repeat(32));
    assert_eq!(
        registry
            .load(Role::Worker, "a", "backend_engineer", &["rust"], 8)
            .unwrap(),
        "x".repeat(32)
    );
    fixture.add("a.md", "a", "backend_engineer", &"x".repeat(40));
    assert_eq!(
        registry.load(Role::Worker, "a", "backend_engineer", &["rust"], 8),
        Err(SkillError::BudgetExceeded)
    );
}

#[test]
fn reload_is_atomic_and_content_is_read_on_demand() {
    let fixture = Fixture::new();
    fixture.add("a.md", "a", "worker", "first");
    let mut registry = SkillRegistry::new(&fixture.0).unwrap();
    fixture.add("a.md", "a", "worker", "second");
    assert_eq!(
        registry
            .load(Role::Worker, "a", "worker", &["axum"], 100)
            .unwrap(),
        "second"
    );
    fixture.add("b.md", "a", "worker", "duplicate");
    assert_eq!(registry.reload(), Err(SkillError::DuplicateId));
    assert_eq!(registry.metadata().len(), 1);
    fs::remove_file(fixture.0.join("b.md")).unwrap();
    fixture.add("b.md", "b", "worker", "new");
    registry.reload().unwrap();
    assert_eq!(registry.metadata().len(), 2);
    fixture.add("b.md", "changed", "worker", "new");
    assert_eq!(
        registry.load(Role::Worker, "b", "worker", &["rust"], 100),
        Err(SkillError::Changed)
    );
    fs::remove_file(fixture.0.join("b.md")).unwrap();
    registry.reload().unwrap();
    assert_eq!(registry.metadata().len(), 1);
    assert_eq!(
        registry.load(Role::Worker, "b", "worker", &["rust"], 100),
        Err(SkillError::NotFound)
    );
}

#[cfg(unix)]
#[test]
fn rejects_symlink_and_oversized_content() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.add("a.md", "a", "worker", "small");
    let registry = SkillRegistry::new(&fixture.0).unwrap();
    fs::remove_file(fixture.0.join("a.md")).unwrap();
    symlink("/etc/passwd", fixture.0.join("a.md")).unwrap();
    assert_eq!(
        registry.load(Role::Worker, "a", "worker", &["rust"], 100),
        Err(SkillError::UnsafePath)
    );
    assert!(matches!(
        SkillRegistry::new(&fixture.0),
        Err(SkillError::UnsafePath)
    ));
    fs::remove_file(fixture.0.join("a.md")).unwrap();
    fixture.add("a.md", "a", "worker", &"x".repeat(65_536));
    let registry = SkillRegistry::new(&fixture.0).unwrap();
    assert_eq!(
        registry.load(Role::Worker, "a", "worker", &["rust"], 100),
        Err(SkillError::TooLarge)
    );

    fs::remove_file(fixture.0.join("a.md")).unwrap();
    symlink("/etc", fixture.0.join("nested")).unwrap();
    assert!(matches!(
        SkillRegistry::new(&fixture.0),
        Err(SkillError::UnsafePath)
    ));
    fs::remove_file(fixture.0.join("nested")).unwrap();
    fs::create_dir(fixture.0.join(".env.backup")).unwrap();
    fixture.add(".env.backup/a.md", "a", "worker", "safe");
    assert!(matches!(
        SkillRegistry::new(&fixture.0),
        Err(SkillError::UnsafePath)
    ));
    fs::remove_dir_all(fixture.0.join(".env.backup")).unwrap();

    fixture.add("a.md", "a", "worker", "safe");
    let registry = SkillRegistry::new(&fixture.0).unwrap();
    let moved = fixture.0.with_extension("old");
    fs::rename(&fixture.0, &moved).unwrap();
    symlink("/etc", &fixture.0).unwrap();
    assert_eq!(
        registry.load(Role::Worker, "a", "worker", &["rust"], 100),
        Err(SkillError::UnsafePath)
    );
    fs::remove_file(&fixture.0).unwrap();
    fs::rename(moved, &fixture.0).unwrap();
}
