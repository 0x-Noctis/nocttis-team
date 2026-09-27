use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Component, Path},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use serde::Serialize;

const MAX_FILES: usize = 128;
const MAX_PATH_BYTES: usize = 256;
const MAX_FILE_BYTES: u64 = 64 * 1024;
const MAX_LIST_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
pub enum DiscoveryError {
    InvalidRepository,
    ListingFailed,
    Io,
}

#[derive(Debug, Default, Eq, PartialEq, Serialize)]
pub struct RepositoryMap {
    pub files: Vec<String>,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub entry_points: Vec<String>,
    pub test_commands: Vec<String>,
    pub config_files: Vec<String>,
    pub instruction_files: Vec<String>,
    pub truncated: bool,
}

/// Memetakan metadata repository tanpa mengirim isi source atau instruksi ke artifact.
pub fn discover(root: &Path) -> Result<RepositoryMap, DiscoveryError> {
    let root = root
        .canonicalize()
        .map_err(|_| DiscoveryError::InvalidRepository)?;
    if !root.is_dir() {
        return Err(DiscoveryError::InvalidRepository);
    }

    let mut child = Command::new("rg")
        .args([
            "--files",
            "--hidden",
            "--null",
            "--no-messages",
            "--glob",
            "!.git",
            "--glob",
            "!**/.git/**",
            ".",
        ])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| DiscoveryError::ListingFailed)?;
    let stdout = child.stdout.take().ok_or(DiscoveryError::ListingFailed)?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut listing = Vec::new();
        let result = stdout.take(MAX_LIST_BYTES + 1).read_to_end(&mut listing);
        let _ = tx.send(result.map(|_| listing));
    });
    let mut listing = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(listing)) => listing,
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DiscoveryError::ListingFailed);
        }
    };
    let listing_truncated = listing.len() as u64 > MAX_LIST_BYTES;
    if listing_truncated {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|_| DiscoveryError::ListingFailed)?;
    if !listing_truncated && !matches!(status.code(), Some(0 | 1)) {
        return Err(DiscoveryError::ListingFailed);
    }
    if listing_truncated {
        listing.truncate(MAX_LIST_BYTES as usize);
        listing.truncate(listing.iter().rposition(|b| *b == 0).map_or(0, |i| i + 1));
    }
    let mut paths = listing
        .split(|b| *b == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| std::str::from_utf8(path).ok())
        .map(|path| path.strip_prefix("./").unwrap_or(path).to_owned())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();

    let mut map = RepositoryMap {
        truncated: listing_truncated,
        ..RepositoryMap::default()
    };
    let mut languages = BTreeSet::new();
    let mut frameworks = BTreeSet::new();
    let mut commands = BTreeSet::new();
    for path in paths {
        if path.len() > MAX_PATH_BYTES || !safe_path(&path) {
            map.truncated = true;
            continue;
        }
        let file = root.join(&path);
        if !regular_without_symlinks(&root, &path)? {
            continue;
        }
        let size = fs::metadata(&file).map_err(|_| DiscoveryError::Io)?.len();
        if size > MAX_FILE_BYTES {
            map.truncated = true;
            continue;
        }
        let mut bytes = Vec::new();
        File::open(&file)
            .map_err(|_| DiscoveryError::Io)?
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| DiscoveryError::Io)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            map.truncated = true;
            continue;
        }
        if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
            continue;
        }
        let name = Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let extension = Path::new(&path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if let Some(language) = match extension {
            "rs" => Some("Rust"),
            "js" | "mjs" | "cjs" => Some("JavaScript"),
            "ts" | "tsx" => Some("TypeScript"),
            "py" => Some("Python"),
            "go" => Some("Go"),
            "svelte" => Some("Svelte"),
            "vue" => Some("Vue"),
            "html" => Some("HTML"),
            _ => None,
        } {
            languages.insert(language.to_owned());
        }
        if matches!(
            name,
            "AGENTS.md" | "Agents.md" | "AGENT.md" | "CLAUDE.md" | "Claude.md"
        ) {
            map.instruction_files.push(path.clone());
        }
        if map.files.len() == MAX_FILES {
            map.truncated = true;
            continue;
        }
        if matches!(
            name,
            "Cargo.toml"
                | "package.json"
                | "pyproject.toml"
                | "go.mod"
                | "Makefile"
                | "vite.config.ts"
                | "vite.config.js"
                | "svelte.config.js"
                | "next.config.js"
                | "next.config.mjs"
                | "next.config.ts"
        ) {
            map.config_files.push(path.clone());
        }
        let local_path = path.split_once('/').map(|(_, rest)| rest).unwrap_or(&path);
        if matches!(
            path.as_str(),
            "src/main.rs" | "src/lib.rs" | "main.py" | "app.py" | "main.go" | "cmd/main.go"
        ) || matches!(
            local_path,
            "src/main.rs"
                | "src/lib.rs"
                | "main.py"
                | "app.py"
                | "main.go"
                | "cmd/main.go"
                | "src/index.js"
                | "src/index.ts"
                | "src/main.js"
                | "src/main.ts"
                | "src/main.tsx"
                | "src/main.jsx"
        ) {
            map.entry_points.push(path.clone());
        }
        if name == "Cargo.toml" {
            languages.insert("Rust".to_owned());
            commands.insert(test_command(&path, "cargo test"));
        }
        if name == "Cargo.toml"
            && let Ok(manifest) =
                toml::from_str::<toml::Value>(std::str::from_utf8(&bytes).unwrap_or(""))
            && manifest
                .get("dependencies")
                .and_then(|v| v.as_table())
                .is_some_and(|deps| deps.contains_key("axum"))
        {
            frameworks.insert("Axum".to_owned());
        }
        if name == "package.json"
            && let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            if manifest
                .get("scripts")
                .and_then(|v| v.get("test"))
                .and_then(|v| v.as_str())
                .is_some()
            {
                commands.insert(test_command(&path, "npm test"));
            }
            for (dependency, label) in [
                ("@sveltejs/kit", "SvelteKit"),
                ("react", "React"),
                ("next", "Next.js"),
                ("vue", "Vue"),
                ("express", "Express"),
                ("vite", "Vite"),
            ] {
                if ["dependencies", "devDependencies"]
                    .iter()
                    .any(|key| manifest.get(key).and_then(|v| v.get(dependency)).is_some())
                {
                    frameworks.insert(label.to_owned());
                }
            }
        }
        if name == "pyproject.toml" {
            commands.insert(test_command(&path, "python -m pytest"));
        }
        if name == "go.mod" {
            commands.insert(test_command(&path, "go test ./..."));
        }
        map.files.push(path);
    }
    map.languages = languages.into_iter().collect();
    map.frameworks = frameworks.into_iter().collect();
    map.test_commands = commands.into_iter().collect();
    Ok(map)
}

fn test_command(manifest: &str, command: &str) -> String {
    let parent = Path::new(manifest).parent().unwrap_or(Path::new(""));
    if parent.as_os_str().is_empty() {
        command.to_owned()
    } else {
        format!("{command} (cwd: {})", parent.display())
    }
}

fn safe_path(path: &str) -> bool {
    !path.chars().any(char::is_control)
        && !path.contains('\\')
        && Path::new(path)
            .components()
            .all(|component| match component {
                Component::Normal(name) => {
                    let name = name.to_string_lossy().to_ascii_lowercase();
                    !matches!(
                        name.as_str(),
                        ".git"
                            | ".env"
                            | ".netrc"
                            | ".npmrc"
                            | ".pypirc"
                            | "credentials"
                            | "credentials.json"
                            | "id_rsa"
                            | "id_ed25519"
                    ) && !name.starts_with(".env.")
                        && !matches!(
                            Path::new(&name).extension().and_then(|e| e.to_str()),
                            Some("pem" | "key" | "p12" | "pfx")
                        )
                }
                _ => false,
            })
}

fn regular_without_symlinks(root: &Path, relative: &str) -> Result<bool, DiscoveryError> {
    let mut file = root.to_path_buf();
    for component in Path::new(relative).components() {
        file.push(component);
        if fs::symlink_metadata(&file)
            .map_err(|_| DiscoveryError::Io)?
            .file_type()
            .is_symlink()
        {
            return Ok(false);
        }
    }
    Ok(fs::symlink_metadata(file)
        .map_err(|_| DiscoveryError::Io)?
        .is_file())
}

#[cfg(test)]
mod tests {
    use super::safe_path;

    #[test]
    fn rejects_escape_and_secret_names() {
        for path in [
            "../outside",
            ".env",
            "nested/id_rsa",
            "nested/key.pem",
            "a\\b",
        ] {
            assert!(!safe_path(path), "{path}");
        }
        assert!(safe_path("web/AGENTS.md"));
    }
}
