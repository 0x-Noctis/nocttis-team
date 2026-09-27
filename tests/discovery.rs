#[path = "../src/context/discovery.rs"]
mod discovery;

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use discovery::discover;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "discovery-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, path: &str, content: &[u8]) {
        let file = self.0.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn discovers_sample_project_without_running_a_model() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample-project/template");
    let map = discover(&root).unwrap();
    assert_eq!(map.languages, ["JavaScript"]);
    assert_eq!(map.test_commands, ["npm test"]);
    assert_eq!(map.config_files, ["package.json"]);
    assert!(map.files.contains(&"src/backend.js".to_owned()));
    assert!(map.files.contains(&"test/project.test.js".to_owned()));
    assert!(!map.truncated);
}

#[test]
fn polyglot_nested_instructions_and_ignored_unsafe_files() {
    let root = Fixture::new();
    root.write(".gitignore", b"ignored.py\n");
    root.write("ignored.py", b"print('ignored')");
    root.write("AGENTS.md", b"root rules");
    root.write("web/AGENTS.md", b"web rules");
    root.write("web/src/main.ts", b"export {};");
    root.write(
        "web/package.json",
        br#"{"scripts":{"test":"node --test"},"devDependencies":{"@sveltejs/kit":"2"}}"#,
    );
    root.write("src/main.rs", b"fn main() {}");
    root.write(
        "Cargo.toml",
        b"[package]\nname = 'sample'\nversion = '0.1.0'\n[dependencies]\naxum = '0.8'\n",
    );
    root.write(".env", b"do-not-index");
    root.write("binary.rs", b"text\0binary");
    root.write("large.rs", &vec![b'x'; 65537]);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("../Cargo.toml", root.0.join("web/linked.toml")).unwrap();
    }
    let first = discover(&root.0).unwrap();
    let second = discover(&root.0).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.languages, ["Rust", "TypeScript"]);
    assert_eq!(first.frameworks, ["Axum", "SvelteKit"]);
    assert_eq!(first.entry_points, ["src/main.rs", "web/src/main.ts"]);
    assert_eq!(first.instruction_files, ["AGENTS.md", "web/AGENTS.md"]);
    assert_eq!(first.test_commands, ["cargo test", "npm test (cwd: web)"]);
    for excluded in [
        "ignored.py",
        ".env",
        "binary.rs",
        "large.rs",
        "web/linked.toml",
    ] {
        assert!(
            !first.files.iter().any(|file| file == excluded),
            "{excluded}"
        );
    }
    assert!(first.truncated);
    let artifact = serde_json::to_string(&first).unwrap();
    assert!(!artifact.contains("do-not-index"));
    assert!(!artifact.contains("web rules"));
}

#[test]
fn caps_file_count_and_rejects_invalid_root() {
    let root = Fixture::new();
    for i in 0..140 {
        root.write(&format!("src/{i:03}.js"), b"export {};");
    }
    root.write("zzz/AGENTS.md", b"late instructions");
    let map = discover(&root.0).unwrap();
    assert_eq!(map.files.len(), 128);
    assert!(map.instruction_files.contains(&"zzz/AGENTS.md".to_owned()));
    assert!(map.truncated);
    assert!(discover(&root.0.join("missing")).is_err());
}
