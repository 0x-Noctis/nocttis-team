//! Retensi dan pembersihan (M5-002): artifact yatim di disk dan worktree cabang integrasi run yang sudah tuntas.
//!
//! Prinsip keselamatan:
//! - Yang direferensikan tidak pernah dihapus. Artifact dianggap direferensikan bila id-nya muncul di tabel
//!   `artifacts`, `tool_call_reservations`, atau payload `events` (bukti verifikasi/tool dirujuk lewat event).
//!   Task `DONE` wajib tetap punya patch dan bukti verifikasi (gerbang rilis M5-012).
//! - Hanya file yang lebih tua dari masa retensi yang disentuh, supaya tulisan yang sedang berlangsung
//!   (file ditulis lebih dulu, baris DB menyusul) tidak terhapus.
//! - Branch integrasi tidak pernah dihapus: hasil kerjanya masih bahan merge manual. Hanya direktori worktree-nya.
//! - Setiap aksi (termasuk dry-run) dicatat di `retention_actions`. Kegagalan satu aksi tidak menghentikan yang lain
//!   dan aman diulang pada putaran berikutnya (idempoten: yang sudah terhapus tidak muncul lagi).

use std::{
    fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    runner::git::GitWorktreeManager,
    store::{
        artifact::{ArtifactError, ArtifactStore},
        task::TaskRepository,
    },
};

#[derive(Clone, Debug)]
pub struct RetentionConfig {
    pub worktree_root: PathBuf,
    /// Umur minimum sebelum artifact yatim atau worktree run yang tuntas boleh dibersihkan.
    pub retention: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionKind {
    OrphanArtifact,
    IntegrationWorktree,
}

impl ActionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::OrphanArtifact => "orphan_artifact",
            Self::IntegrationWorktree => "integration_worktree",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Deleted,
    WouldDelete,
    Failed,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::WouldDelete => "would_delete",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Action {
    pub kind: ActionKind,
    pub target: String,
    pub outcome: Outcome,
    pub detail: Option<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub dry_run: bool,
    pub actions: Vec<Action>,
    /// Baris `artifacts` yang file-nya hilang. Hanya dilaporkan: menghapus baris akan menghilangkan jejak audit.
    pub missing_files: Vec<String>,
}

#[derive(Debug)]
pub enum RetentionError {
    Database(sqlx::Error),
    Artifact(ArtifactError),
    Io(std::io::Error),
}

impl std::fmt::Display for RetentionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "retention database error: {error}"),
            Self::Artifact(error) => write!(formatter, "retention artifact error: {error}"),
            Self::Io(error) => write!(formatter, "retention io error: {error}"),
        }
    }
}

impl std::error::Error for RetentionError {}

impl From<sqlx::Error> for RetentionError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<ArtifactError> for RetentionError {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

/// Satu putaran retensi. `now` dapat diatur supaya pembersihan bisa diuji tanpa menunggu waktu nyata.
pub async fn run(
    pool: &PgPool,
    artifacts: &ArtifactStore,
    config: &RetentionConfig,
    dry_run: bool,
    now: SystemTime,
) -> Result<Report, RetentionError> {
    let cutoff = now.checked_sub(config.retention).unwrap_or(UNIX_EPOCH);
    let mut report = Report {
        dry_run,
        ..Report::default()
    };
    orphan_artifacts(pool, artifacts, cutoff, dry_run, &mut report).await?;
    missing_artifact_files(pool, artifacts, &mut report).await?;
    integration_worktrees(pool, config, cutoff, dry_run, &mut report).await?;
    for action in &report.actions {
        sqlx::query("INSERT INTO retention_actions (dry_run,kind,target,outcome,detail) VALUES ($1,$2,$3,$4,$5)")
            .bind(dry_run)
            .bind(action.kind.as_str())
            .bind(&action.target)
            .bind(action.outcome.as_str())
            .bind(&action.detail)
            .execute(pool)
            .await?;
    }
    Ok(report)
}

async fn referenced(pool: &PgPool, artifact_id: &str) -> Result<bool, sqlx::Error> {
    // `position` (bukan LIKE) supaya `_` dan `%` di id tidak berperan sebagai wildcard.
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM artifacts WHERE id::text=$1)
             OR EXISTS (SELECT 1 FROM tool_call_reservations WHERE artifact_id::text=$1)
             OR EXISTS (SELECT 1 FROM events WHERE position($1 in payload::text) > 0)",
    )
    .bind(artifact_id)
    .fetch_one(pool)
    .await
}

async fn orphan_artifacts(
    pool: &PgPool,
    artifacts: &ArtifactStore,
    cutoff: SystemTime,
    dry_run: bool,
    report: &mut Report,
) -> Result<(), RetentionError> {
    for (id, modified) in artifacts.list()? {
        if modified > cutoff || referenced(pool, &id).await? {
            continue;
        }
        let (outcome, detail) = if dry_run {
            (Outcome::WouldDelete, None)
        } else {
            let removed = artifacts.remove(&id);
            // `remove` mengabaikan kegagalan hapus file; pastikan benar-benar hilang sebelum menyatakan berhasil.
            match removed {
                Ok(()) if !artifacts.contains(&id) => (Outcome::Deleted, None),
                Ok(()) => (
                    Outcome::Failed,
                    Some("file masih ada setelah dihapus".to_owned()),
                ),
                Err(error) => (Outcome::Failed, Some(error.to_string())),
            }
        };
        report.actions.push(Action {
            kind: ActionKind::OrphanArtifact,
            target: id,
            outcome,
            detail,
        });
    }
    Ok(())
}

async fn missing_artifact_files(
    pool: &PgPool,
    artifacts: &ArtifactStore,
    report: &mut Report,
) -> Result<(), RetentionError> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id::text FROM artifacts ORDER BY created_at LIMIT 10000")
            .fetch_all(pool)
            .await?;
    report.missing_files = ids
        .into_iter()
        .filter(|id| !artifacts.contains(id))
        .collect();
    Ok(())
}

/// Worktree `integration-<run>` dibersihkan bila semua task run itu `DONE`/`CANCELLED` dan yang terakhir berubah
/// lebih lama dari masa retensi. Run dengan task yang masih bisa bergerak (mis. NEEDS_HUMAN) dibiarkan.
async fn integration_worktrees(
    pool: &PgPool,
    config: &RetentionConfig,
    cutoff: SystemTime,
    dry_run: bool,
    report: &mut Report,
) -> Result<(), RetentionError> {
    let entries = match fs::read_dir(&config.worktree_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(RetentionError::Io(error)),
    };
    let cutoff_epoch = cutoff
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let tasks = TaskRepository::new(pool.clone());
    for entry in entries {
        let name = entry
            .map_err(RetentionError::Io)?
            .file_name()
            .to_string_lossy()
            .into_owned();
        let Some(run) = name
            .strip_prefix("integration-")
            .and_then(|id| Uuid::parse_str(id).ok())
            .filter(|id| id.to_string() == name["integration-".len()..])
        else {
            continue;
        };
        let state = sqlx::query(
            "SELECT count(*) AS total,
                    count(*) FILTER (WHERE status NOT IN ('DONE','CANCELLED')) AS open,
                    COALESCE(max(updated_at) < to_timestamp($2), false) AS old
             FROM tasks WHERE project_run_id=$1",
        )
        .bind(run)
        .bind(cutoff_epoch)
        .fetch_one(pool)
        .await?;
        if state.get::<i64, _>("total") == 0
            || state.get::<i64, _>("open") > 0
            || !state.get::<bool, _>("old")
        {
            continue;
        }
        let outcome = if dry_run {
            (Outcome::WouldDelete, None)
        } else {
            match remove_integration_worktree(pool, &tasks, config, run).await {
                Ok(()) => (Outcome::Deleted, None),
                Err(detail) => (Outcome::Failed, Some(detail)),
            }
        };
        report.actions.push(Action {
            kind: ActionKind::IntegrationWorktree,
            target: run.to_string(),
            outcome: outcome.0,
            detail: outcome.1,
        });
    }
    Ok(())
}

async fn remove_integration_worktree(
    pool: &PgPool,
    tasks: &TaskRepository,
    config: &RetentionConfig,
    run: Uuid,
) -> Result<(), String> {
    let target_id = format!("integration-{run}");
    let base = tasks
        .integration_target_base(&target_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("tidak ada catatan integrasi untuk run ini")?;
    let repository: String = sqlx::query_scalar(
        "SELECT p.repository_path FROM project_runs pr JOIN projects p ON p.id=pr.project_id WHERE pr.id=$1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .map_err(|error| error.to_string())?;
    let git = GitWorktreeManager::new(repository, &config.worktree_root)
        .map_err(|error| error.to_string())?;
    let worktree = git
        .open(&target_id, &format!("noctis-integration-{run}"), &base)
        .map_err(|error| error.to_string())?;
    git.remove_worktree_keep_branch(&worktree)
        .map_err(|error| error.to_string())
}
