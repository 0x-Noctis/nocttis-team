#[path = "../src/store/artifact.rs"]
mod artifact;

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use artifact::{ArtifactError, ArtifactStore};

static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "noctis-artifact-store-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn writes_reads_and_returns_sha256_metadata() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();
    let metadata = store
        .write(
            "result-1",
            "report.json",
            "application/json",
            b"hello",
            |_| Ok::<_, ()>(()),
        )
        .unwrap();

    assert_eq!(store.read("result-1").unwrap(), b"hello");
    assert_eq!(store.metadata("result-1").unwrap(), metadata);
    assert_eq!(metadata.size, 5);
    assert_eq!(
        metadata.checksum,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
}

#[test]
fn bounded_read_enforces_caller_limit_against_actual_file() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();
    store
        .write("bounded", "report.txt", "text/plain", b"12345", |_| {
            Ok::<_, ()>(())
        })
        .unwrap();

    assert_eq!(
        store.read_bounded("bounded", 4),
        Err(ArtifactError::TooLarge)
    );
    assert_eq!(store.read_bounded("bounded", 5).unwrap(), b"12345");
}

#[test]
fn rejects_oversized_and_unsafe_names_without_partial_files() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 4).unwrap();

    assert_eq!(
        store.write(
            "large",
            "large.bin",
            "application/octet-stream",
            b"12345",
            |_| Ok::<_, ()>(())
        ),
        Err(ArtifactError::TooLarge)
    );
    for unsafe_name in ["../escape", "/absolute", "a/b", "a\\b", ".."] {
        assert_eq!(
            store.write(
                unsafe_name,
                "safe.txt",
                "text/plain",
                b"x",
                |_| Ok::<_, ()>(())
            ),
            Err(ArtifactError::InvalidArtifactId)
        );
    }
    assert_eq!(
        store.write("safe", "../escape", "text/plain", b"x", |_| Ok::<_, ()>(())),
        Err(ArtifactError::InvalidLogicalName)
    );
    assert!(fs::read_dir(&directory.0).unwrap().next().is_none());
}

#[test]
fn rejects_duplicate_without_overwriting() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();
    store
        .write("same", "one.txt", "text/plain", b"first", |_| {
            Ok::<_, ()>(())
        })
        .unwrap();

    assert_eq!(
        store.write("same", "two.txt", "text/plain", b"second", |_| Ok::<_, ()>(
            ()
        )),
        Err(ArtifactError::AlreadyExists)
    );
    assert_eq!(store.read("same").unwrap(), b"first");
}

#[test]
fn metadata_failure_removes_final_and_partial_files() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();

    assert_eq!(
        store.write("failed", "failed.txt", "text/plain", b"data", |_| Err(
            "database unavailable"
        )),
        Err(ArtifactError::MetadataCallbackFailed)
    );
    assert_eq!(store.read("failed"), Err(ArtifactError::NotFound));
    assert!(fs::read_dir(&directory.0).unwrap().next().is_none());
}

#[test]
fn write_failure_leaves_no_partial_file() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();
    fs::remove_dir(&directory.0).unwrap();

    assert!(matches!(
        store.write(
            "failed",
            "failed.txt",
            "text/plain",
            b"data",
            |_| Ok::<_, ()>(())
        ),
        Err(ArtifactError::UnsafePath | ArtifactError::Io(_))
    ));
    assert!(!directory.0.exists());
}

#[cfg(unix)]
#[test]
fn rejects_symlink_escape() {
    use std::os::unix::fs::symlink;

    let directory = TestDirectory::new();
    let outside = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();
    let destination = directory.0.join("escape.artifact");
    symlink(outside.0.join("stolen"), &destination).unwrap();

    assert_eq!(
        store.write(
            "escape",
            "safe.txt",
            "text/plain",
            b"secret",
            |_| Ok::<_, ()>(())
        ),
        Err(ArtifactError::UnsafePath)
    );
    assert!(!outside.0.join("stolen").exists());
    assert_eq!(
        fs::read_link(destination).unwrap(),
        outside.0.join("stolen")
    );
}

#[test]
fn errors_do_not_expose_host_paths() {
    let directory = TestDirectory::new();
    let store = ArtifactStore::new(&directory.0, 1024).unwrap();
    fs::remove_dir(&directory.0).unwrap();
    let error = store.read("missing").unwrap_err().to_string();

    assert!(!error.contains(directory.0.to_string_lossy().as_ref()));
}
