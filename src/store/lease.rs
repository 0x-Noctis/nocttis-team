//! File lease per project run (M4-002).
//!
//! Lease mencegah dua task/attempt mengedit scope yang beririsan secara bersamaan. Unique constraint
//! `(project_run_id, path_pattern)` hanya menangkap pola yang persis sama, sedangkan overlap glob
//! (`src/**` vs `src/api/x.rs`) harus dihitung di aplikasi. Karena itu cek-lalu-simpan dijalankan dalam
//! satu transaksi yang diserialkan per run lewat advisory lock transaksi (singkat, dilepas saat commit);
//! run berbeda tidak saling menunggu.

use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::path_scope::{PathScope, ScopeError};

const MAX_SCOPES: usize = 64;
const MAX_TTL_SECONDS: i64 = 3_600;

#[derive(Debug)]
pub enum LeaseError {
    /// Scope bentrok dengan lease aktif milik owner lain; tidak ada lease yang berubah.
    Conflict {
        holder_task_id: String,
        pattern: String,
    },
    /// Lease sudah tidak dimiliki owner ini (kedaluwarsa, direbut, atau sudah dilepas).
    Lost,
    InvalidScope(ScopeError),
    InvalidInput(&'static str),
    Database(sqlx::Error),
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { .. } => formatter.write_str("file scope is leased by another task"),
            Self::Lost => formatter.write_str("file lease is no longer held"),
            Self::InvalidScope(error) => error.fmt(formatter),
            Self::InvalidInput(field) => write!(formatter, "invalid lease input: {field}"),
            Self::Database(_) => formatter.write_str("database operation failed"),
        }
    }
}

impl std::error::Error for LeaseError {}

impl From<sqlx::Error> for LeaseError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub pattern: String,
    pub task_id: String,
    pub owner: Uuid,
}

#[derive(Clone)]
pub struct LeaseStore {
    pool: PgPool,
}

impl LeaseStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Ambil semua `patterns` sekaligus atau tidak sama sekali. Lease kedaluwarsa dibuang lebih dulu
    /// (stale lease bisa direbut). Owner yang sama boleh mengambil ulang scope yang beririsan dengan
    /// miliknya sendiri (idempotent, sekaligus memperpanjang). Mengembalikan pola kanonik yang dipegang.
    pub async fn acquire(
        &self,
        run_id: Uuid,
        task_id: &str,
        owner: Uuid,
        patterns: &[&str],
        ttl_seconds: i64,
    ) -> Result<Vec<String>, LeaseError> {
        if task_id.trim().is_empty() {
            return Err(LeaseError::InvalidInput("task_id"));
        }
        if !(1..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(LeaseError::InvalidInput("ttl_seconds"));
        }
        if patterns.is_empty() || patterns.len() > MAX_SCOPES {
            return Err(LeaseError::InvalidInput("patterns"));
        }
        let mut wanted: Vec<PathScope> = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            wanted.push(PathScope::parse(pattern).map_err(LeaseError::InvalidScope)?);
        }

        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('file_leases:' || $1::text, 0))",
        )
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM file_leases WHERE project_run_id=$1 AND expires_at <= now()")
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        let held = sqlx::query(
            "SELECT path_pattern,task_id FROM file_leases WHERE project_run_id=$1 AND owner<>$2",
        )
        .bind(run_id)
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        for row in &held {
            let pattern: String = row.get("path_pattern");
            // Baris yang tidak bisa diparse dianggap bentrok dengan segalanya (gagal aman).
            let theirs = PathScope::parse(&pattern).unwrap_or(PathScope::Subtree(String::new()));
            if wanted.iter().any(|mine| mine.overlaps(&theirs)) {
                return Err(LeaseError::Conflict {
                    holder_task_id: row.get("task_id"),
                    pattern,
                });
            }
        }
        let mut canonical: Vec<String> = wanted.iter().map(PathScope::pattern).collect();
        canonical.sort();
        canonical.dedup();
        for pattern in &canonical {
            sqlx::query(
                "INSERT INTO file_leases (project_run_id,path_pattern,task_id,owner,expires_at)
                 VALUES ($1,$2,$3,$4,now()+($5 * interval '1 second'))
                 ON CONFLICT (project_run_id,path_pattern) DO UPDATE
                 SET expires_at=EXCLUDED.expires_at,renewed_at=now(),task_id=EXCLUDED.task_id
                 WHERE file_leases.owner=EXCLUDED.owner",
            )
            .bind(run_id)
            .bind(pattern)
            .bind(task_id)
            .bind(owner)
            .bind(ttl_seconds)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(canonical)
    }

    /// Perpanjang semua lease milik owner. `Lost` bila tidak ada satu pun yang masih aktif:
    /// pemanggil harus berhenti mengedit karena scope-nya mungkin sudah dipegang task lain.
    pub async fn renew(
        &self,
        run_id: Uuid,
        owner: Uuid,
        ttl_seconds: i64,
    ) -> Result<u64, LeaseError> {
        if !(1..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(LeaseError::InvalidInput("ttl_seconds"));
        }
        let renewed = sqlx::query(
            "UPDATE file_leases SET expires_at=now()+($3 * interval '1 second'),renewed_at=now()
             WHERE project_run_id=$1 AND owner=$2 AND expires_at > now()",
        )
        .bind(run_id)
        .bind(owner)
        .bind(ttl_seconds)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if renewed == 0 {
            Err(LeaseError::Lost)
        } else {
            Ok(renewed)
        }
    }

    /// Lepas semua lease milik owner; idempotent. Mengembalikan jumlah baris yang dilepas.
    pub async fn release(&self, run_id: Uuid, owner: Uuid) -> Result<u64, LeaseError> {
        Ok(
            sqlx::query("DELETE FROM file_leases WHERE project_run_id=$1 AND owner=$2")
                .bind(run_id)
                .bind(owner)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Lepas semua lease sebuah task tanpa memeriksa owner. Dipakai recovery setelah attempt dinyatakan
    /// mati, supaya attempt pengganti tidak menunggu lease kedaluwarsa.
    pub async fn release_task(&self, task_id: &str) -> Result<u64, LeaseError> {
        Ok(sqlx::query("DELETE FROM file_leases WHERE task_id=$1")
            .bind(task_id)
            .execute(&self.pool)
            .await?
            .rows_affected())
    }

    /// Lease aktif (belum kedaluwarsa) satu run, untuk dashboard dan pengujian.
    pub async fn active(&self, run_id: Uuid) -> Result<Vec<Lease>, LeaseError> {
        let rows = sqlx::query(
            "SELECT path_pattern,task_id,owner FROM file_leases
             WHERE project_run_id=$1 AND expires_at > now() ORDER BY path_pattern",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| Lease {
                pattern: row.get("path_pattern"),
                task_id: row.get("task_id"),
                owner: row.get("owner"),
            })
            .collect())
    }
}
