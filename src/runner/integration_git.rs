//! Cabang integrasi tingkat run (M4-006): satu worktree + branch `noctis-integration-<run>` yang dibuat
//! dari commit dasar tertentu. Patch task diterapkan satu per satu dan di-commit di branch ini saja;
//! branch dasar, working tree repository, dan remote tidak pernah disentuh (tidak ada `git push`).
//!
//! Alur per patch: `stage_patch` (terapkan ke index tanpa commit) -> pemeriksa memeriksa hasilnya ->
//! `commit` atau `discard`. Karena patch belum di-commit selama diperiksa, rollback cukup `discard`.

use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    thread,
};

use crate::runner::{
    git::{GitError, GitWorktreeManager, Worktree},
    policy::validate_relative,
};

const OUTPUT_CAP: usize = 64 * 1024;
const MAX_MARKER_SCAN_BYTES: u64 = 1024 * 1024;
const MAX_DETAIL_CHARS: usize = 2_000;
// Identitas commit tetap dan hook dimatikan: commit integrasi tidak boleh bergantung pada konfigurasi pengguna.
const COMMIT_ARGS: [&str; 8] = [
    "-c",
    "user.name=Noctis Integrator",
    "-c",
    "user.email=integrator@noctis.invalid",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.hooksPath=/dev/null",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageOutcome {
    /// Patch ada di index. `three_way` = baris konteks sudah bergeser sehingga perlu merge tiga arah.
    Staged { files: Vec<String>, three_way: bool },
    /// Patch tidak bisa diterapkan secara mekanis; worktree sudah dibersihkan seperti semula.
    Rejected { files: Vec<String>, detail: String },
}

pub struct IntegrationBranch {
    worktree: Worktree,
}

struct Output {
    ok: bool,
    stdout: Vec<u8>,
    stderr: String,
}

impl IntegrationBranch {
    /// Buat branch dan worktree integrasi dari `base_commit` untuk satu run.
    pub fn create(
        manager: &GitWorktreeManager,
        run_id: &str,
        base_commit: &str,
    ) -> Result<Self, GitError> {
        let worktree = manager.create(
            &format!("integration-{run_id}"),
            &format!("noctis-integration-{run_id}"),
            base_commit,
        )?;
        Ok(Self { worktree })
    }

    pub fn path(&self) -> &Path {
        self.worktree.path()
    }

    pub fn branch(&self) -> &str {
        self.worktree.branch()
    }

    pub fn base_commit(&self) -> &str {
        self.worktree.base_commit()
    }

    pub fn head(&self) -> Result<String, GitError> {
        let output = self.git(&["rev-parse", "HEAD"], None)?;
        require(&output, "could not read integration HEAD")?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// True bila tidak ada perubahan yang belum di-commit (termasuk file untracked).
    pub fn is_clean(&self) -> Result<bool, GitError> {
        let output = self.git(&["status", "--porcelain=v1", "--untracked-files=all"], None)?;
        require(&output, "could not read integration status")?;
        Ok(output.stdout.is_empty())
    }

    /// Terapkan patch ke index dan working tree integrasi tanpa commit. Mencoba apply langsung dulu;
    /// bila konteks sudah bergeser dicoba merge tiga arah. Hasil yang gagal selalu dibersihkan.
    pub fn stage_patch(&self, patch: &[u8]) -> Result<StageOutcome, GitError> {
        if !self.is_clean()? {
            return Err(GitError::InvalidInput("integration worktree is dirty"));
        }
        let files: Vec<String> = patch_files(patch)?.into_iter().collect();
        let direct = ["apply", "--recount", "--whitespace=nowarn"];
        let check = self.git(&[&direct[..], &["--check", "-"]].concat(), Some(patch))?;
        if check.ok {
            let applied = self.git(&[&direct[..], &["--index", "-"]].concat(), Some(patch))?;
            if applied.ok {
                return Ok(StageOutcome::Staged {
                    files,
                    three_way: false,
                });
            }
            self.discard()?;
            return Ok(rejected(files, &applied, self.path()));
        }
        let merged = self.git(&[&direct[..], &["--3way", "-"]].concat(), Some(patch))?;
        if merged.ok {
            return Ok(StageOutcome::Staged {
                files,
                three_way: true,
            });
        }
        // Konflik tiga arah meninggalkan penanda konflik di working tree; kembalikan ke HEAD.
        self.discard()?;
        Ok(rejected(files, &merged, self.path()))
    }

    /// Buang semua perubahan yang belum di-commit (rollback satu patch).
    pub fn discard(&self) -> Result<(), GitError> {
        require(
            &self.git(&["reset", "--hard", "-q", "HEAD"], None)?,
            "could not reset integration worktree",
        )?;
        require(
            &self.git(&["clean", "-fdq"], None)?,
            "could not clean integration worktree",
        )
    }

    /// Commit patch yang sedang di-stage dan kembalikan hash commit-nya.
    pub fn commit(&self, message: &str) -> Result<String, GitError> {
        if message.trim().is_empty() || message.contains('\0') {
            return Err(GitError::InvalidInput("commit message is invalid"));
        }
        let output = self.git(
            &[
                &COMMIT_ARGS[..],
                &["commit", "-q", "--no-verify", "-m", message],
            ]
            .concat(),
            None,
        )?;
        require(&output, "could not commit integration patch")?;
        self.head()
    }

    /// Kembalikan branch integrasi ke commit sebelumnya (mis. gerbang akhir gagal). Commit harus leluhur HEAD.
    pub fn rollback_to(&self, commit: &str) -> Result<(), GitError> {
        if commit.len() < 7 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(GitError::InvalidInput("commit is invalid"));
        }
        let ancestor = self.git(&["merge-base", "--is-ancestor", commit, "HEAD"], None)?;
        if !ancestor.ok {
            return Err(GitError::InvalidInput(
                "rollback target is not an ancestor of integration HEAD",
            ));
        }
        require(
            &self.git(&["reset", "--hard", "-q", commit], None)?,
            "could not roll back integration branch",
        )?;
        require(
            &self.git(&["clean", "-fdq"], None)?,
            "could not clean integration worktree",
        )
    }

    /// File (relatif) yang berisi penanda konflik utuh (`<<<<<<< ` ... `>>>>>>> `). Pasangan ini dipakai,
    /// bukan `=======` saja, karena garis itu sah di Markdown.
    pub fn conflict_markers(&self, files: &[String]) -> Vec<String> {
        files
            .iter()
            .filter(|file| {
                let path = self.path().join(file);
                let Ok(metadata) = fs::metadata(&path) else {
                    return false;
                };
                if !metadata.is_file() || metadata.len() > MAX_MARKER_SCAN_BYTES {
                    return false;
                }
                // File biner (bukan UTF-8) tidak memuat penanda teks.
                let Ok(text) = fs::read_to_string(&path) else {
                    return false;
                };
                let mut open = false;
                text.lines().any(|line| {
                    if line == "<<<<<<<" || line.starts_with("<<<<<<< ") {
                        open = true;
                    }
                    open && (line == ">>>>>>>" || line.starts_with(">>>>>>> "))
                })
            })
            .cloned()
            .collect()
    }

    fn git(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<Output, GitError> {
        run_git(self.path(), args, stdin)
    }
}

fn require(output: &Output, message: &'static str) -> Result<(), GitError> {
    if output.ok {
        Ok(())
    } else {
        Err(GitError::InvalidInput(message))
    }
}

fn rejected(files: Vec<String>, output: &Output, root: &Path) -> StageOutcome {
    StageOutcome::Rejected {
        files,
        detail: sanitize(&output.stderr, root),
    }
}

/// Potong dan bersihkan pesan Git: tanpa path absolut worktree dan tanpa karakter kontrol.
fn sanitize(text: &str, root: &Path) -> String {
    text.replace(&root.display().to_string(), "<integration>")
        .chars()
        .map(|character| {
            if character.is_control() && character != '\n' {
                ' '
            } else {
                character
            }
        })
        .take(MAX_DETAIL_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn run_git(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<Output, GitError> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| GitError::Io)?;
    // Patch ditulis dari thread terpisah supaya pipa stdout/stderr tidak membuntu.
    let writer = stdin.map(|input| {
        let mut pipe = child.stdin.take().expect("stdin was piped");
        let input = input.to_vec();
        thread::spawn(move || pipe.write_all(&input))
    });
    let output = child.wait_with_output().map_err(|_| GitError::Io)?;
    if let Some(writer) = writer {
        // Git yang menolak patch lebih awal menutup pipa; itu bukan error I/O kita.
        let _ = writer.join();
    }
    let mut stdout = output.stdout;
    stdout.truncate(OUTPUT_CAP);
    let mut stderr = output.stderr;
    stderr.truncate(OUTPUT_CAP);
    Ok(Output {
        ok: output.status.success(),
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Path file yang disentuh patch (`git diff --binary --no-renames`). Menolak patch kosong, path yang
/// dikutip (karakter tak lazim), rename/copy, serta path absolut, `..`, atau bagian `.git`.
pub fn patch_files(patch: &[u8]) -> Result<BTreeSet<String>, GitError> {
    let mut files = BTreeSet::new();
    for line in patch.split(|byte| *byte == b'\n') {
        let Some(rest) = line.strip_prefix(b"diff --git ") else {
            continue;
        };
        let rest = std::str::from_utf8(rest)
            .map_err(|_| GitError::InvalidInput("patch path is not UTF-8"))?;
        if rest.starts_with('"') {
            return Err(GitError::InvalidInput(
                "patch paths with special characters are not supported",
            ));
        }
        // `a/<p> b/<p>`: panjangnya 2*len(p) + 5; kedua sisi harus sama (tidak ada rename).
        let length = rest
            .len()
            .checked_sub(5)
            .filter(|length| length % 2 == 0)
            .map(|length| length / 2);
        let path = length
            .and_then(|length| rest.get(2..2 + length))
            .filter(|path| rest == format!("a/{path} b/{path}"))
            .ok_or(GitError::InvalidInput(
                "renames and copies are not supported",
            ))?;
        let normalized =
            validate_relative(path).map_err(|_| GitError::InvalidInput("patch path is invalid"))?;
        if Path::new(&normalized)
            .components()
            .any(|component| component.as_os_str() == ".git")
        {
            return Err(GitError::InvalidInput("patch touches .git"));
        }
        files.insert(normalized);
    }
    if files.is_empty() {
        return Err(GitError::InvalidInput("patch has no files"));
    }
    Ok(files)
}
