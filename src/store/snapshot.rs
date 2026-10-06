//! Snapshot status scheduler satu run untuk dashboard (M4-007B): slot aktif, antrean beserta alasan
//! menunggu, lease, konflik, attempt, dan budget. Semua dibaca dari tabel yang sama dengan yang dipakai
//! scheduler (`agent_runs`, `tasks`, `file_leases`, `budget_reservations`, `model_usage`), dalam satu
//! transaksi REPEATABLE READ supaya angka antar bagian konsisten. Tidak ada yang ditulis.

use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::{
    budget::{BudgetLevel, level, project_reserve},
    path_scope::PathScope,
};

const MAX_ATTEMPTS_LISTED: i64 = 100;
const MAX_TASK_GAUGES: i64 = 50;

#[derive(Debug)]
pub enum SnapshotError {
    NotFound,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for SnapshotError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Gauge {
    pub label: String,
    pub limit: i64,
    pub used: i64,
    pub held: i64,
    pub estimated: bool,
    pub reserve: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SlotLease {
    pub pattern: String,
    pub expires_in_seconds: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SlotAttempt {
    pub number: i64,
    pub max: i64,
    pub branch: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Slot {
    pub slot: usize,
    /// running | pausing | cancelling; slot kosong tidak dikirim (UI mengisinya sampai `max_slots`).
    pub state: &'static str,
    pub task_id: String,
    pub title: String,
    pub task_status: String,
    pub attempt: SlotAttempt,
    pub heartbeat_age_seconds: i64,
    pub leases: Vec<SlotLease>,
    pub tokens: Gauge,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct QueueItem {
    pub task_id: String,
    pub title: String,
    pub priority: i32,
    /// slot | dependency | lease | budget | paused | attempts
    pub reason: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RunLease {
    pub pattern: String,
    pub task_id: String,
    pub expires_in_seconds: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct LeaseConflict {
    pub task_id: String,
    pub pattern: String,
    pub holder_task_id: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttemptRow {
    pub task_id: String,
    pub number: i64,
    pub status: String,
    pub error_code: Option<String>,
    pub tokens: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RunSnapshot {
    pub run_id: Uuid,
    pub run_status: String,
    pub max_slots: usize,
    pub budget: Gauge,
    pub slots: Vec<Slot>,
    pub queue: Vec<QueueItem>,
    pub leases: Vec<RunLease>,
    pub conflicts: Vec<LeaseConflict>,
    pub attempts: Vec<AttemptRow>,
    pub task_budgets: Vec<Gauge>,
}

pub async fn run_snapshot(
    pool: &PgPool,
    run_id: Uuid,
    max_slots: usize,
) -> Result<RunSnapshot, SnapshotError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;

    let run =
        sqlx::query("SELECT status,token_budget,reserved_tokens FROM project_runs WHERE id=$1")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(SnapshotError::NotFound)?;
    let run_status: String = run.get("status");
    let (limit, reserved_tokens): (i64, i64) =
        (run.get("token_budget"), run.get("reserved_tokens"));

    // Budget run: usage tercatat + reservasi yang ditahan, sama dengan perhitungan BudgetStore.
    let usage = sqlx::query(
        "SELECT COALESCE(SUM(u.input_tokens+u.output_tokens),0)::bigint AS used,
                COALESCE(bool_or(u.estimated),false) AS estimated
         FROM model_usage u JOIN agent_runs a ON a.id=u.agent_run_id JOIN tasks t ON t.id=a.task_id
         WHERE t.project_run_id=$1",
    )
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    let held: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(input_tokens+output_tokens),0)::bigint FROM budget_reservations WHERE project_run_id=$1 AND status='held'",
    )
    .bind(run_id)
    .fetch_one(&mut *tx)
    .await?;
    let reserve = project_reserve(limit);
    let budget = Gauge {
        label: "Run budget".to_owned(),
        limit,
        used: usage.get("used"),
        held,
        estimated: usage.get("estimated"),
        reserve,
    };
    let guard_stopped = level(
        budget.used.saturating_add(held),
        limit.saturating_sub(reserve),
    ) == BudgetLevel::Stop;

    let leases: Vec<RunLease> = sqlx::query(
        "SELECT path_pattern,task_id,GREATEST(EXTRACT(EPOCH FROM (expires_at-now())),0)::bigint AS remaining
         FROM file_leases WHERE project_run_id=$1 AND expires_at > now() ORDER BY path_pattern",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| RunLease { pattern: row.get("path_pattern"), task_id: row.get("task_id"), expires_in_seconds: row.get("remaining") })
    .collect();

    // Slot = attempt aktif, diurut menurut waktu mulai.
    let rows = sqlx::query(
        "SELECT a.id,a.task_id,t.title,t.status AS task_status,a.attempt,t.max_attempts,COALESCE(a.branch,'') AS branch,
                GREATEST(EXTRACT(EPOCH FROM (now()-COALESCE(a.heartbeat_at,a.started_at))),0)::bigint AS beat,
                t.max_input_tokens,t.max_output_tokens,
                COALESCE((SELECT SUM(u.input_tokens+u.output_tokens) FROM model_usage u WHERE u.agent_run_id=a.id),0)::bigint AS used,
                COALESCE((SELECT SUM(r.input_tokens+r.output_tokens) FROM budget_reservations r WHERE r.agent_run_id=a.id AND r.status='held'),0)::bigint AS held,
                COALESCE((SELECT bool_or(u.estimated) FROM model_usage u WHERE u.agent_run_id=a.id),false) AS estimated
         FROM agent_runs a JOIN tasks t ON t.id=a.task_id
         WHERE t.project_run_id=$1 AND a.finished_at IS NULL AND a.status IN ('assigned','running')
         ORDER BY a.started_at,a.id",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?;
    let state = match run_status.as_str() {
        "PAUSED" => "pausing",
        "CANCELLED" => "cancelling",
        _ => "running",
    };
    let slots: Vec<Slot> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let task_id: String = row.get("task_id");
            Slot {
                slot: index + 1,
                state,
                title: row.get("title"),
                task_status: row.get("task_status"),
                attempt: SlotAttempt {
                    number: row.get("attempt"),
                    max: i64::from(row.get::<i16, _>("max_attempts")),
                    branch: row.get("branch"),
                },
                heartbeat_age_seconds: row.get("beat"),
                leases: leases
                    .iter()
                    .filter(|lease| lease.task_id == task_id)
                    .map(|lease| SlotLease {
                        pattern: lease.pattern.clone(),
                        expires_in_seconds: lease.expires_in_seconds,
                    })
                    .collect(),
                tokens: Gauge {
                    label: task_id.clone(),
                    limit: row
                        .get::<i64, _>("max_input_tokens")
                        .saturating_add(row.get("max_output_tokens")),
                    used: row.get("used"),
                    held: row.get("held"),
                    estimated: row.get("estimated"),
                    reserve: 0,
                },
                task_id,
            }
        })
        .collect();

    // Antrean: semua task READY dengan alasan pertama yang menghalangi, sesuai urutan syarat claim.
    let ready = sqlx::query(
        "SELECT t.id,t.title,t.priority,t.allowed_paths,t.plan_id,t.max_input_tokens,t.max_output_tokens,t.max_attempts,
                (SELECT count(*) FROM agent_runs ar WHERE ar.task_id=t.id) AS tried,
                COALESCE((SELECT jsonb_agg(jsonb_build_object('id',d.dependency_id,'status',req.status) ORDER BY d.dependency_id)
                          FROM task_dependencies d JOIN tasks req ON req.id=d.dependency_id
                          WHERE d.task_id=t.id AND req.status<>'DONE'),'[]'::jsonb) AS blockers
         FROM tasks t WHERE t.project_run_id=$1 AND t.status='READY' ORDER BY t.priority DESC,t.created_at,t.id",
    )
    .bind(run_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut queue = Vec::with_capacity(ready.len());
    let mut conflicts = Vec::new();
    for row in &ready {
        let task_id: String = row.get("id");
        let needed = row
            .get::<i64, _>("max_input_tokens")
            .saturating_add(row.get("max_output_tokens"));
        let paths: Vec<String> =
            serde_json::from_value(row.get("allowed_paths")).unwrap_or_default();
        let blockers: Vec<serde_json::Value> =
            serde_json::from_value(row.get("blockers")).unwrap_or_default();
        let (reason, detail) = if run_status == "PAUSED" {
            ("paused", "Resume the run to schedule this task.".to_owned())
        } else if !blockers.is_empty() {
            let names: Vec<String> = blockers
                .iter()
                .map(|blocker| {
                    format!(
                        "{} ({})",
                        blocker["id"].as_str().unwrap_or("?"),
                        blocker["status"].as_str().unwrap_or("?").to_lowercase()
                    )
                })
                .collect();
            ("dependency", format!("Waiting for {}.", names.join(", ")))
        } else if row.get::<i64, _>("tried") >= i64::from(row.get::<i16, _>("max_attempts")) {
            (
                "attempts",
                format!(
                    "No attempts left ({} of {} used).",
                    row.get::<i64, _>("tried"),
                    row.get::<i16, _>("max_attempts")
                ),
            )
        } else if let Some((holder, pattern)) = lease_conflict(&paths, &task_id, &leases) {
            conflicts.push(LeaseConflict {
                task_id: task_id.clone(),
                pattern: pattern.clone(),
                holder_task_id: holder.clone(),
            });
            ("lease", format!("{pattern} is leased by {holder}."))
        } else if guard_stopped {
            (
                "budget",
                "The run budget has reached its limit for new work.".to_owned(),
            )
        } else if row.get::<Option<String>, _>("plan_id").is_some() && reserved_tokens < needed {
            (
                "budget",
                format!(
                    "The plan reserved {reserved_tokens} tokens, less than the {needed} this task needs."
                ),
            )
        } else if row.get::<Option<String>, _>("plan_id").is_none()
            && reserved_tokens.saturating_add(needed) > limit
        {
            (
                "budget",
                format!(
                    "Needs {needed} tokens; {} remain unreserved.",
                    limit.saturating_sub(reserved_tokens).max(0)
                ),
            )
        } else if slots.len() >= max_slots {
            ("slot", format!("All {max_slots} worker slots are busy."))
        } else {
            (
                "slot",
                "Ready; it starts on the next scheduler tick.".to_owned(),
            )
        };
        queue.push(QueueItem {
            task_id,
            title: row.get("title"),
            priority: row.get("priority"),
            reason,
            detail,
        });
    }

    let attempts: Vec<AttemptRow> = sqlx::query(
        "SELECT a.task_id,a.attempt,a.status,a.error_code,
                COALESCE((SELECT SUM(u.input_tokens+u.output_tokens) FROM model_usage u WHERE u.agent_run_id=a.id),0)::bigint AS tokens
         FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE t.project_run_id=$1
         ORDER BY a.task_id,a.attempt LIMIT $2",
    )
    .bind(run_id)
    .bind(MAX_ATTEMPTS_LISTED)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| AttemptRow {
        task_id: row.get("task_id"),
        number: row.get("attempt"),
        status: row.get("status"),
        error_code: row.get("error_code"),
        tokens: row.get("tokens"),
    })
    .collect();

    // Budget per task: batas semua attempt; hanya task yang sudah memakai atau menahan token.
    let task_budgets: Vec<Gauge> = sqlx::query(
        "SELECT t.id,(t.max_input_tokens+t.max_output_tokens)*t.max_attempts AS task_limit,
                COALESCE(SUM(u.input_tokens+u.output_tokens),0)::bigint AS used,COALESCE(bool_or(u.estimated),false) AS estimated,
                COALESCE((SELECT SUM(r.input_tokens+r.output_tokens) FROM budget_reservations r WHERE r.task_id=t.id AND r.status='held'),0)::bigint AS held
         FROM tasks t JOIN agent_runs a ON a.task_id=t.id LEFT JOIN model_usage u ON u.agent_run_id=a.id
         WHERE t.project_run_id=$1 GROUP BY t.id,t.max_input_tokens,t.max_output_tokens,t.max_attempts
         HAVING COALESCE(SUM(u.input_tokens+u.output_tokens),0) > 0
         ORDER BY t.id LIMIT $2",
    )
    .bind(run_id)
    .bind(MAX_TASK_GAUGES)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| Gauge { label: row.get("id"), limit: row.get("task_limit"), used: row.get("used"), held: row.get("held"), estimated: row.get("estimated"), reserve: 0 })
    .collect();

    tx.commit().await?;
    Ok(RunSnapshot {
        run_id,
        run_status,
        max_slots,
        budget,
        slots,
        queue,
        leases,
        conflicts,
        attempts,
        task_budgets,
    })
}

/// Lease task lain yang beririsan dengan scope task ini (konservatif, sama dengan LeaseStore).
fn lease_conflict(
    paths: &[String],
    task_id: &str,
    leases: &[RunLease],
) -> Option<(String, String)> {
    let wanted: Vec<PathScope> = paths
        .iter()
        .filter_map(|path| PathScope::parse(path).ok())
        .collect();
    leases
        .iter()
        .filter(|lease| lease.task_id != task_id)
        .find_map(|lease| {
            let held = PathScope::parse(&lease.pattern).ok()?;
            wanted
                .iter()
                .any(|scope| scope.overlaps(&held))
                .then(|| (lease.task_id.clone(), lease.pattern.clone()))
        })
}
