use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactMetadata {
    pub artifact_id: String,
    pub logical_name: String,
    pub media_type: String,
    pub size: u64,
    pub checksum: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactError {
    InvalidArtifactId,
    InvalidLogicalName,
    InvalidMediaType,
    TooLarge,
    AlreadyExists,
    NotFound,
    UnsafePath,
    InvalidMetadata,
    MetadataCallbackFailed,
    Io(&'static str),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidArtifactId => "invalid artifact ID",
            Self::InvalidLogicalName => "invalid logical name",
            Self::InvalidMediaType => "invalid media type",
            Self::TooLarge => "artifact exceeds maximum size",
            Self::AlreadyExists => "artifact already exists",
            Self::NotFound => "artifact not found",
            Self::UnsafePath => "unsafe artifact path",
            Self::InvalidMetadata => "invalid artifact metadata",
            Self::MetadataCallbackFailed => "artifact metadata callback failed",
            Self::Io(operation) => operation,
        })
    }
}

impl std::error::Error for ArtifactError {}

pub struct ArtifactStore {
    root: PathBuf,
    maximum_size: u64,
    writes: Mutex<()>,
}

impl ArtifactStore {
    pub fn new(root: impl AsRef<Path>, maximum_size: u64) -> Result<Self, ArtifactError> {
        fs::create_dir_all(root.as_ref())
            .map_err(|_| ArtifactError::Io("create artifact store"))?;
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|_| ArtifactError::Io("open artifact store"))?;
        if !root.is_dir() {
            return Err(ArtifactError::UnsafePath);
        }
        Ok(Self {
            root,
            maximum_size,
            writes: Mutex::new(()),
        })
    }

    pub fn write<F, E>(
        &self,
        artifact_id: &str,
        logical_name: &str,
        media_type: &str,
        bytes: &[u8],
        persist_metadata: F,
    ) -> Result<ArtifactMetadata, ArtifactError>
    where
        F: FnOnce(&ArtifactMetadata) -> Result<(), E>,
    {
        validate_component(artifact_id).map_err(|_| ArtifactError::InvalidArtifactId)?;
        validate_component(logical_name).map_err(|_| ArtifactError::InvalidLogicalName)?;
        if media_type.is_empty() || media_type.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(ArtifactError::InvalidMediaType);
        }
        let size = u64::try_from(bytes.len()).map_err(|_| ArtifactError::TooLarge)?;
        if size > self.maximum_size {
            return Err(ArtifactError::TooLarge);
        }

        let _write = self
            .writes
            .lock()
            .map_err(|_| ArtifactError::Io("lock artifact store"))?;
        self.ensure_root()?;
        let lock_path = self.root.join(format!(".{artifact_id}.lock"));
        let lock = ArtifactLock::acquire(lock_path)?;
        let artifact_path = self.artifact_path(artifact_id);
        let metadata_path = self.metadata_path(artifact_id);
        ensure_destination_available(&artifact_path)?;
        ensure_destination_available(&metadata_path)?;

        let metadata = ArtifactMetadata {
            artifact_id: artifact_id.to_owned(),
            logical_name: logical_name.to_owned(),
            media_type: media_type.to_owned(),
            size,
            checksum: hex(&Sha256::digest(bytes)),
        };
        let metadata_bytes = serde_json::to_vec(&metadata)
            .map_err(|_| ArtifactError::Io("serialize artifact metadata"))?;
        let artifact_temp = self.temp_path(artifact_id, "artifact");
        let metadata_temp = self.temp_path(artifact_id, "metadata");

        if let Err(error) = write_new_file(&artifact_temp, bytes) {
            remove_if_present(&artifact_temp);
            return Err(error);
        }
        if let Err(error) = write_new_file(&metadata_temp, &metadata_bytes) {
            remove_if_present(&artifact_temp);
            remove_if_present(&metadata_temp);
            return Err(error);
        }
        if fs::rename(&artifact_temp, &artifact_path).is_err() {
            remove_if_present(&artifact_temp);
            remove_if_present(&metadata_temp);
            return Err(ArtifactError::Io("finalize artifact"));
        }
        if fs::rename(&metadata_temp, &metadata_path).is_err() {
            remove_if_present(&artifact_path);
            remove_if_present(&metadata_temp);
            return Err(ArtifactError::Io("finalize artifact metadata"));
        }
        if persist_metadata(&metadata).is_err() {
            remove_if_present(&artifact_path);
            remove_if_present(&metadata_path);
            return Err(ArtifactError::MetadataCallbackFailed);
        }
        drop(lock);
        Ok(metadata)
    }

    /// True bila salah satu file artifact (isi atau metadata) masih ada di disk.
    pub fn contains(&self, artifact_id: &str) -> bool {
        validate_component(artifact_id).is_ok()
            && (fs::symlink_metadata(self.artifact_path(artifact_id)).is_ok()
                || fs::symlink_metadata(self.metadata_path(artifact_id)).is_ok())
    }

    /// Daftar artifact yang ada di disk beserta waktu ubah paling awal dari file-nya, termasuk yang hanya punya
    /// salah satu file (artifact atau metadata). File sementara/lock (berawalan titik) dan nama tak valid dilewati.
    pub fn list(&self) -> Result<Vec<(String, std::time::SystemTime)>, ArtifactError> {
        self.ensure_root()?;
        let mut found: std::collections::BTreeMap<String, std::time::SystemTime> =
            std::collections::BTreeMap::new();
        for entry in fs::read_dir(&self.root).map_err(|_| ArtifactError::Io("list artifacts"))? {
            let entry = entry.map_err(|_| ArtifactError::Io("list artifacts"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name
                .strip_suffix(".metadata.json")
                .or_else(|| name.strip_suffix(".artifact"))
            else {
                continue;
            };
            if name.starts_with('.') || validate_component(id).is_err() {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            found
                .entry(id.to_owned())
                .and_modify(|current| *current = (*current).min(modified))
                .or_insert(modified);
        }
        Ok(found.into_iter().collect())
    }

    pub fn read(&self, artifact_id: &str) -> Result<Vec<u8>, ArtifactError> {
        self.read_bounded(artifact_id, self.maximum_size)
    }

    pub fn remove(&self, artifact_id: &str) -> Result<(), ArtifactError> {
        validate_component(artifact_id).map_err(|_| ArtifactError::InvalidArtifactId)?;
        let _write = self
            .writes
            .lock()
            .map_err(|_| ArtifactError::Io("lock artifact store"))?;
        self.ensure_root()?;
        remove_if_present(&self.artifact_path(artifact_id));
        remove_if_present(&self.metadata_path(artifact_id));
        Ok(())
    }

    pub fn read_bounded(
        &self,
        artifact_id: &str,
        maximum_size: u64,
    ) -> Result<Vec<u8>, ArtifactError> {
        validate_component(artifact_id).map_err(|_| ArtifactError::InvalidArtifactId)?;
        self.ensure_root()?;
        let path = self.artifact_path(artifact_id);
        ensure_regular_file(&path)?;
        let file = File::open(&path).map_err(|_| ArtifactError::Io("read artifact"))?;
        let size_limit = maximum_size.min(self.maximum_size);
        if file
            .metadata()
            .map_err(|_| ArtifactError::Io("read artifact"))?
            .len()
            > size_limit
        {
            return Err(ArtifactError::TooLarge);
        }
        let mut bytes = Vec::new();
        file.take(size_limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| ArtifactError::Io("read artifact"))?;
        if bytes.len() as u64 > size_limit {
            return Err(ArtifactError::TooLarge);
        }
        Ok(bytes)
    }

    pub fn metadata(&self, artifact_id: &str) -> Result<ArtifactMetadata, ArtifactError> {
        validate_component(artifact_id).map_err(|_| ArtifactError::InvalidArtifactId)?;
        self.ensure_root()?;
        let path = self.metadata_path(artifact_id);
        ensure_regular_file(&path)?;
        let bytes = fs::read(path).map_err(|_| ArtifactError::Io("read artifact metadata"))?;
        let metadata: ArtifactMetadata =
            serde_json::from_slice(&bytes).map_err(|_| ArtifactError::InvalidMetadata)?;
        if metadata.artifact_id != artifact_id {
            return Err(ArtifactError::InvalidMetadata);
        }
        Ok(metadata)
    }

    fn ensure_root(&self) -> Result<(), ArtifactError> {
        let metadata = fs::symlink_metadata(&self.root).map_err(|_| ArtifactError::UnsafePath)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ArtifactError::UnsafePath);
        }
        let canonical = self
            .root
            .canonicalize()
            .map_err(|_| ArtifactError::UnsafePath)?;
        if canonical != self.root {
            return Err(ArtifactError::UnsafePath);
        }
        Ok(())
    }

    fn artifact_path(&self, artifact_id: &str) -> PathBuf {
        self.root.join(format!("{artifact_id}.artifact"))
    }

    fn metadata_path(&self, artifact_id: &str) -> PathBuf {
        self.root.join(format!("{artifact_id}.metadata.json"))
    }

    fn temp_path(&self, artifact_id: &str, kind: &str) -> PathBuf {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!(
            ".{artifact_id}.{kind}.{}.{}.tmp",
            std::process::id(),
            sequence
        ))
    }
}

struct ArtifactLock(PathBuf);

impl ArtifactLock {
    fn acquire(path: PathBuf) -> Result<Self, ArtifactError> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    ArtifactError::AlreadyExists
                } else {
                    ArtifactError::Io("lock artifact")
                }
            })?;
        Ok(Self(path))
    }
}

impl Drop for ArtifactLock {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            remove_if_present(&self.0);
        }
    }
}

fn validate_component(value: &str) -> Result<(), ()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(());
    }
    Ok(())
}

fn ensure_destination_available(path: &Path) -> Result<(), ArtifactError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ArtifactError::UnsafePath),
        Ok(_) => Err(ArtifactError::AlreadyExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ArtifactError::Io("inspect artifact destination")),
    }
}

fn ensure_regular_file(path: &Path) -> Result<(), ArtifactError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ArtifactError::UnsafePath),
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(ArtifactError::UnsafePath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ArtifactError::NotFound),
        Err(_) => Err(ArtifactError::Io("inspect artifact")),
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ArtifactError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| ArtifactError::Io("create temporary artifact"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| ArtifactError::Io("write temporary artifact"))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn remove_if_present(path: &Path) {
    let _ = fs::remove_file(path);
}
