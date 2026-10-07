//! Kesehatan operasional dan metrik (M5-003). Semua angka dihitung dari database saat diminta, bukan
//! disimpan sebagai counter di memori, sehingga tetap benar setelah restart dan konsisten antar proses.
//!
//! Aturan label metrik: hanya nilai dari himpunan tetap (`TASK_STATUSES`, `ATTEMPT_STATUSES`, nama komponen).
//! ID task/run/provider tidak pernah menjadi label, supaya jumlah deret tidak meledak.

use std::{collections::HashMap, fmt::Write as _, sync::LazyLock, time::Instant};

use serde::Serialize;
use sqlx::{PgPool, Row, migrate::Migrator};

/// Migration yang tertanam di binary; readiness membandingkan database dengan daftar ini.
pub static MIGRATOR: Migrator = sqlx::migrate!();

pub const TASK_STATUSES: [&str; 15] = [
    "DRAFT",
    "PLANNED",
    "READY",
    "ASSIGNED",
    "RUNNING",
    "SELF_CHECK",
    "REVIEW",
    "CHANGES_REQUESTED",
    "VERIFY",
    "FAILED",
    "INTEGRATE",
    "CONFLICT",
    "NEEDS_HUMAN",
    "DONE",
    "CANCELLED",
];
pub const ATTEMPT_STATUSES: [&str; 5] = [
    "assigned",
    "running",
    "recovery_required",
    "completed",
    "failed",
];

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Mencatat waktu mulai proses; panggil sekali saat startup supaya uptime akurat.
pub fn mark_started() {
    LazyLock::force(&STARTED);
}

pub fn uptime_seconds() -> u64 {
    STARTED.elapsed().as_secs()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Ok,
    /// Berjalan tetapi ada yang perlu perhatian; tidak membuat service tidak siap.
    Degraded,
    Down,
}

impl Component {
    fn gauge(self) -> u8 {
        match self {
            Self::Ok => 2,
            Self::Degraded => 1,
            Self::Down => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Readiness {
    pub database: Component,
    pub migrations: Component,
    pub workers: Component,
    pub providers: Component,
}

impl Readiness {
    /// Siap melayani hanya bila database dan migration sehat. Worker/provider yang bermasalah hanya "degraded":
    /// API tetap berguna (mis. untuk melihat status dan menyetujui plan) walau model belum siap.
    pub fn ready(&self) -> bool {
        self.database == Component::Ok && self.migrations == Component::Ok
    }
}

/// Angka mentah untuk `/metrics`.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub tasks: HashMap<String, i64>,
    pub attempts: HashMap<String, i64>,
    pub stale_attempts: i64,
    pub models: i64,
    pub models_tools_verified: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub estimated_tokens: i64,
    pub migrations_pending: i64,
}

/// Migration sehat bila setiap migration binary sudah terpasang sukses dengan checksum sama dan database tidak
/// memuat versi yang tidak dikenal binary (database lebih baru atau salah). Mengembalikan juga jumlah yang belum terpasang.
pub async fn migrations_state(pool: &PgPool) -> (Component, i64) {
    let rows = match sqlx::query("SELECT version,success,checksum FROM _sqlx_migrations")
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        // Tabel belum ada atau query gagal: migration belum pernah dijalankan.
        Err(_) => return (Component::Down, MIGRATOR.iter().count() as i64),
    };
    let applied: HashMap<i64, (bool, Vec<u8>)> = rows
        .iter()
        .map(|row| {
            (
                row.get("version"),
                (row.get("success"), row.get("checksum")),
            )
        })
        .collect();
    let mut pending = 0;
    let mut healthy = true;
    for migration in MIGRATOR
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
    {
        match applied.get(&migration.version) {
            None => {
                pending += 1;
                healthy = false;
            }
            Some((success, checksum)) => {
                if !success || checksum.as_slice() != migration.checksum.as_ref() {
                    healthy = false;
                }
            }
        }
    }
    let known = MIGRATOR.iter().map(|m| m.version).collect::<Vec<_>>();
    if applied.keys().any(|version| !known.contains(version)) {
        healthy = false;
    }
    (
        if healthy {
            Component::Ok
        } else {
            Component::Down
        },
        pending,
    )
}

/// Periksa komponen. Tidak menyentuh provider eksternal: liveness/readiness tidak boleh bergantung padanya.
pub async fn check(pool: &PgPool, stale_after_seconds: i64) -> Readiness {
    let database = if sqlx::query("SELECT 1").execute(pool).await.is_ok() {
        Component::Ok
    } else {
        Component::Down
    };
    if database == Component::Down {
        return Readiness {
            database,
            migrations: Component::Down,
            workers: Component::Down,
            providers: Component::Down,
        };
    }
    let (migrations, _) = migrations_state(pool).await;
    let workers = match stale_attempts(pool, stale_after_seconds).await {
        Ok(0) => Component::Ok,
        Ok(_) => Component::Degraded,
        Err(_) => Component::Down,
    };
    let providers = match sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM models WHERE verified_capabilities->>'tools'='supported'",
    )
    .fetch_one(pool)
    .await
    {
        Ok(0) => Component::Degraded,
        Ok(_) => Component::Ok,
        Err(_) => Component::Down,
    };
    Readiness {
        database,
        migrations,
        workers,
        providers,
    }
}

async fn stale_attempts(pool: &PgPool, stale_after_seconds: i64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM agent_runs
         WHERE status IN ('assigned','running') AND finished_at IS NULL
           AND heartbeat_at < now() - make_interval(secs => $1::double precision)",
    )
    .bind(stale_after_seconds as f64)
    .fetch_one(pool)
    .await
}

pub async fn snapshot(pool: &PgPool, stale_after_seconds: i64) -> Result<Snapshot, sqlx::Error> {
    let counts = |rows: Vec<sqlx::postgres::PgRow>| -> HashMap<String, i64> {
        rows.iter()
            .map(|row| (row.get::<String, _>(0), row.get::<i64, _>(1)))
            .collect()
    };
    let tasks = counts(
        sqlx::query("SELECT status::text,count(*) FROM tasks GROUP BY 1")
            .fetch_all(pool)
            .await?,
    );
    let attempts = counts(
        sqlx::query("SELECT status::text,count(*) FROM agent_runs GROUP BY 1")
            .fetch_all(pool)
            .await?,
    );
    let models = sqlx::query(
        "SELECT count(*),count(*) FILTER (WHERE verified_capabilities->>'tools'='supported') FROM models",
    )
    .fetch_one(pool)
    .await?;
    let usage = sqlx::query(
        "SELECT COALESCE(sum(input_tokens),0)::bigint,COALESCE(sum(output_tokens),0)::bigint,
                COALESCE(sum(input_tokens+output_tokens) FILTER (WHERE estimated),0)::bigint FROM model_usage",
    )
    .fetch_one(pool)
    .await?;
    Ok(Snapshot {
        tasks,
        attempts,
        stale_attempts: stale_attempts(pool, stale_after_seconds).await?,
        models: models.get(0),
        models_tools_verified: models.get(1),
        input_tokens: usage.get(0),
        output_tokens: usage.get(1),
        estimated_tokens: usage.get(2),
        migrations_pending: migrations_state(pool).await.1,
    })
}

fn gauge(out: &mut String, name: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
}

/// Format teks Prometheus. `snapshot` kosong (None) berarti database tidak terjangkau: hanya seri proses yang
/// ditulis, dan `noctis_component_health{component="database"}` bernilai 0.
pub fn render(readiness: &Readiness, snapshot: Option<&Snapshot>) -> String {
    let mut out = String::new();
    gauge(
        &mut out,
        "noctis_up",
        "Proses hidup (selalu 1 bila endpoint ini menjawab).",
    );
    out.push_str("noctis_up 1\n");
    gauge(&mut out, "noctis_build_info", "Versi build.");
    let _ = writeln!(
        out,
        "noctis_build_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    gauge(
        &mut out,
        "noctis_uptime_seconds",
        "Detik sejak proses mulai.",
    );
    let _ = writeln!(out, "noctis_uptime_seconds {}", uptime_seconds());
    gauge(
        &mut out,
        "noctis_component_health",
        "Kesehatan komponen: 2 ok, 1 degraded, 0 down.",
    );
    for (name, component) in [
        ("database", readiness.database),
        ("migrations", readiness.migrations),
        ("workers", readiness.workers),
        ("providers", readiness.providers),
    ] {
        let _ = writeln!(
            out,
            "noctis_component_health{{component=\"{name}\"}} {}",
            component.gauge()
        );
    }
    let Some(snapshot) = snapshot else {
        return out;
    };
    gauge(&mut out, "noctis_tasks", "Jumlah task per status.");
    for status in TASK_STATUSES {
        let _ = writeln!(
            out,
            "noctis_tasks{{status=\"{status}\"}} {}",
            snapshot.tasks.get(status).copied().unwrap_or(0)
        );
    }
    gauge(&mut out, "noctis_attempts", "Jumlah attempt per status.");
    for status in ATTEMPT_STATUSES {
        let _ = writeln!(
            out,
            "noctis_attempts{{status=\"{status}\"}} {}",
            snapshot.attempts.get(status).copied().unwrap_or(0)
        );
    }
    gauge(
        &mut out,
        "noctis_stale_attempts",
        "Attempt aktif yang heartbeat-nya melewati batas stale.",
    );
    let _ = writeln!(out, "noctis_stale_attempts {}", snapshot.stale_attempts);
    gauge(&mut out, "noctis_models", "Jumlah model terdaftar.");
    let _ = writeln!(out, "noctis_models {}", snapshot.models);
    gauge(
        &mut out,
        "noctis_models_tools_verified",
        "Model dengan kemampuan tools terverifikasi.",
    );
    let _ = writeln!(
        out,
        "noctis_models_tools_verified {}",
        snapshot.models_tools_verified
    );
    gauge(
        &mut out,
        "noctis_tokens",
        "Token tercatat; `estimated` = bagian yang berupa estimasi.",
    );
    for (kind, value) in [
        ("input", snapshot.input_tokens),
        ("output", snapshot.output_tokens),
        ("estimated", snapshot.estimated_tokens),
    ] {
        let _ = writeln!(out, "noctis_tokens{{kind=\"{kind}\"}} {value}");
    }
    gauge(
        &mut out,
        "noctis_migrations_pending",
        "Migration yang belum terpasang.",
    );
    let _ = writeln!(
        out,
        "noctis_migrations_pending {}",
        snapshot.migrations_pending
    );
    out
}
