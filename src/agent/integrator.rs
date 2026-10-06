//! Integrator (M4-006): menerapkan patch task yang sudah disetujui reviewer dan lolos verifier ke cabang
//! integrasi, berurutan, dengan pemeriksaan integrasi setelah tiap patch.
//!
//! Integrator hanya MENGUSULKAN status (seperti Reviewer dan Verifier); control plane yang menerapkannya
//! lewat transisi `Actor::Integrator`. Modul ini tidak menyentuh database, branch dasar, maupun remote.

use std::{collections::HashSet, path::Path, time::Duration};

use crate::{
    agent::{
        reviewer::{ReviewDecision, ReviewOutcome},
        verifier::{VerificationReport, VerificationVerdict},
    },
    domain::task::TaskStatus,
    runner::{
        git::GitError,
        integration_git::{IntegrationBranch, StageOutcome, patch_files},
        policy::{ToolPolicy, ToolRole},
    },
};

const MAX_PATCHES: usize = 256;
const MAX_PATCH_BYTES: usize = 1024 * 1024;
const MAX_SUMMARY_CHARS: usize = 2_000;

#[derive(Debug)]
pub enum IntegratorError {
    /// Bukti review/verifikasi tidak menyatakan task layak diintegrasikan.
    NotApprovedAndVerified,
    InvalidPatch(&'static str),
    TooManyPatches,
    DuplicateTask,
    /// Cabang integrasi tidak bersih di awal; mungkin ada run integrasi lain atau sisa kegagalan.
    DirtyBranch,
    Git(GitError),
}

impl std::fmt::Display for IntegratorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotApprovedAndVerified => {
                formatter.write_str("task is not approved and verified")
            }
            Self::InvalidPatch(reason) => write!(formatter, "invalid patch: {reason}"),
            Self::TooManyPatches => formatter.write_str("too many patches in one integration"),
            Self::DuplicateTask => formatter.write_str("a task appears twice in one integration"),
            Self::DirtyBranch => formatter.write_str("integration branch has uncommitted changes"),
            Self::Git(error) => write!(formatter, "integration git failure: {error}"),
        }
    }
}

impl std::error::Error for IntegratorError {}

impl From<GitError> for IntegratorError {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

/// Patch task yang BUKTINYA sudah diperiksa saat konstruksi: tidak ada jalan lain membuat nilai ini.
#[derive(Clone, Debug)]
pub struct ApprovedPatch {
    task_id: String,
    depends_on: Vec<String>,
    patch: Vec<u8>,
    allowed_paths: Vec<String>,
}

impl ApprovedPatch {
    /// `review` harus `Approved` menuju VERIFY dan `verification` harus `Integrate` menuju INTEGRATE dengan
    /// semua perintah lulus; selain itu patch tidak boleh masuk integrasi.
    pub fn new(
        task_id: impl Into<String>,
        depends_on: Vec<String>,
        patch: Vec<u8>,
        allowed_paths: Vec<String>,
        review: &ReviewOutcome,
        verification: &VerificationReport,
    ) -> Result<Self, IntegratorError> {
        let approved = matches!(review.decision, ReviewDecision::Approved)
            && review.next_status == TaskStatus::Verify;
        let verified = verification.verdict == VerificationVerdict::Integrate
            && verification.proposed_status == TaskStatus::Integrate
            && !verification.results.is_empty()
            && verification
                .results
                .iter()
                .all(|result| result.passed && !result.timed_out);
        if !approved || !verified {
            return Err(IntegratorError::NotApprovedAndVerified);
        }
        let task_id = task_id.into();
        if task_id.trim().is_empty() {
            return Err(IntegratorError::InvalidPatch("task id is empty"));
        }
        if patch.is_empty() || patch.len() > MAX_PATCH_BYTES {
            return Err(IntegratorError::InvalidPatch("patch is empty or too large"));
        }
        if allowed_paths.is_empty() {
            return Err(IntegratorError::InvalidPatch("allowed paths are empty"));
        }
        Ok(Self {
            task_id,
            depends_on,
            patch,
            allowed_paths,
        })
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }
}

/// Hasil pemeriksaan integrasi (build/test) pada worktree integrasi setelah satu patch diterapkan.
pub struct CheckResult {
    pub passed: bool,
    /// Ringkasan aman untuk laporan konflik (tanpa secret).
    pub summary: String,
}

/// Pemeriksaan integrasi. Produksi menjalankan test/build project di container; pengujian memakai fungsi biasa.
pub trait IntegrationCheck {
    fn check(&self, worktree: &Path) -> CheckResult;
}

impl<F: Fn(&Path) -> CheckResult> IntegrationCheck for F {
    fn check(&self, worktree: &Path) -> CheckResult {
        self(worktree)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// Patch tidak bisa diterapkan, bahkan dengan merge tiga arah.
    Mechanical,
    /// Patch diterapkan tetapi meninggalkan penanda konflik di file.
    ConflictMarkers,
    /// Patch menyentuh file di luar allowed paths task.
    ScopeViolation,
    /// Patch diterapkan bersih tetapi pemeriksaan integrasi gagal: bertabrakan secara semantik.
    RegressionFailed,
    /// Patch tidak dapat dibaca (path tak valid, rename, dsb.).
    InvalidPatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictReport {
    pub task_id: String,
    pub kind: ConflictKind,
    pub files: Vec<String>,
    /// Penjelasan terpotong dan tanpa path absolut.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskIntegration {
    Integrated {
        commit: String,
        three_way: bool,
    },
    Conflict(ConflictReport),
    /// Dilewati karena dependency di batch ini tidak terintegrasi; tetap INTEGRATE sampai dependency beres.
    Skipped {
        blocked_by: String,
    },
}

impl TaskIntegration {
    /// Urutan status yang harus ditempuh control plane (setiap langkah divalidasi state machine).
    pub fn transitions(&self) -> Vec<TaskStatus> {
        match self {
            Self::Integrated { .. } => vec![TaskStatus::Done],
            Self::Conflict(_) => vec![TaskStatus::Conflict, TaskStatus::NeedsHuman],
            Self::Skipped { .. } => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrationReport {
    pub branch: String,
    pub base_commit: String,
    pub head_commit: String,
    pub results: Vec<(String, TaskIntegration)>,
}

pub struct Integrator;

impl Integrator {
    /// Terapkan `patches` ke `branch` sesuai urutan yang diberikan (pemanggil menyediakan urutan topologis).
    /// Kegagalan satu task tidak menghentikan task lain yang tidak bergantung padanya. Dependency yang tidak
    /// ada di batch dianggap sudah terintegrasi sebelumnya.
    pub fn integrate(
        branch: &IntegrationBranch,
        patches: &[ApprovedPatch],
        check: &dyn IntegrationCheck,
    ) -> Result<IntegrationReport, IntegratorError> {
        if patches.len() > MAX_PATCHES {
            return Err(IntegratorError::TooManyPatches);
        }
        let mut seen = HashSet::new();
        if !patches
            .iter()
            .all(|patch| seen.insert(patch.task_id.as_str()))
        {
            return Err(IntegratorError::DuplicateTask);
        }
        if !branch.is_clean()? {
            return Err(IntegratorError::DirtyBranch);
        }
        let in_batch: HashSet<&str> = patches.iter().map(|patch| patch.task_id.as_str()).collect();
        let mut integrated: HashSet<String> = HashSet::new();
        let mut results = Vec::with_capacity(patches.len());
        for patch in patches {
            let blocked = patch.depends_on.iter().find(|dependency| {
                in_batch.contains(dependency.as_str()) && !integrated.contains(*dependency)
            });
            let outcome = match blocked {
                Some(dependency) => TaskIntegration::Skipped {
                    blocked_by: dependency.clone(),
                },
                None => Self::integrate_one(branch, patch, check)?,
            };
            if matches!(outcome, TaskIntegration::Integrated { .. }) {
                integrated.insert(patch.task_id.clone());
            }
            results.push((patch.task_id.clone(), outcome));
        }
        Ok(IntegrationReport {
            branch: branch.branch().to_owned(),
            base_commit: branch.base_commit().to_owned(),
            head_commit: branch.head()?,
            results,
        })
    }

    fn integrate_one(
        branch: &IntegrationBranch,
        patch: &ApprovedPatch,
        check: &dyn IntegrationCheck,
    ) -> Result<TaskIntegration, IntegratorError> {
        let conflict = |kind, files: Vec<String>, detail: String| {
            TaskIntegration::Conflict(ConflictReport {
                task_id: patch.task_id.clone(),
                kind,
                files,
                detail: detail.chars().take(MAX_SUMMARY_CHARS).collect(),
            })
        };
        // 1. Scope: aturan yang sama dengan tool worker (ToolPolicy), dihitung dari path di dalam patch itu sendiri.
        let files = match patch_files(&patch.patch) {
            Ok(files) => files,
            Err(error) => {
                return Ok(conflict(
                    ConflictKind::InvalidPatch,
                    Vec::new(),
                    error.to_string(),
                ));
            }
        };
        let policy = ToolPolicy::new(
            ToolRole::Worker,
            patch.allowed_paths.clone(),
            MAX_PATCH_BYTES,
            MAX_PATCH_BYTES,
            Duration::from_secs(1),
        )
        .map_err(|_| IntegratorError::InvalidPatch("allowed paths are invalid"))?;
        let outside: Vec<String> = files
            .iter()
            .filter(|file| policy.authorize_path(file).is_err())
            .cloned()
            .collect();
        if !outside.is_empty() {
            let detail = format!(
                "patch touches files outside allowed paths: {}",
                outside.join(", ")
            );
            return Ok(conflict(ConflictKind::ScopeViolation, outside, detail));
        }
        // 2. Terapkan ke index; kegagalan mekanis sudah dibersihkan oleh stage_patch.
        let (files, three_way) = match branch.stage_patch(&patch.patch)? {
            StageOutcome::Staged { files, three_way } => (files, three_way),
            StageOutcome::Rejected { files, detail } => {
                return Ok(conflict(ConflictKind::Mechanical, files, detail));
            }
        };
        // 3. Penanda konflik di hasil, lalu pemeriksaan integrasi; keduanya membatalkan patch ini saja.
        let marked = branch.conflict_markers(&files);
        if !marked.is_empty() {
            branch.discard()?;
            return Ok(conflict(
                ConflictKind::ConflictMarkers,
                marked,
                "applied patch leaves conflict markers".to_owned(),
            ));
        }
        let result = check.check(branch.path());
        if !result.passed {
            branch.discard()?;
            return Ok(conflict(
                ConflictKind::RegressionFailed,
                files,
                result.summary,
            ));
        }
        let commit = branch.commit(&format!("integrate {}", patch.task_id))?;
        Ok(TaskIntegration::Integrated { commit, three_way })
    }
}
