//! Antrean keputusan manusia (M4-009B): plan yang menunggu persetujuan, task yang diserahkan ke manusia
//! (NEEDS_HUMAN/CONFLICT) beserta konteksnya, dan riwayat keputusan lengkap dengan siapa yang memutuskan.
//! Hanya membaca, dalam satu transaksi REPEATABLE READ.

use serde::Serialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

const MAX_PENDING: i64 = 100;
const MAX_ATTENTION: i64 = 50;
const MAX_HISTORY: i64 = 20;
const EVENTS_PER_TASK: i64 = 8;

#[derive(Debug, Serialize, PartialEq)]
pub struct PendingPlan {
    pub plan_id: String,
    pub run_id: Uuid,
    pub project_name: String,
    pub run_objective: String,
    pub run_status: String,
    pub version: i64,
    pub task_count: usize,
    pub risk_flags: Vec<String>,
    /// Token yang akan dipesan saat approve: Σ (max_input + max_output) per task.
    pub reserved_tokens: i64,
    /// Sisa budget run (limit - reserved) saat ini.
    pub available_tokens: i64,
    /// True bila approve pasti ditolak karena reservasi melebihi sisa budget.
    pub over_budget: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ContextEvent {
    pub id: i64,
    pub event_type: String,
    pub actor: String,
    pub actor_id: Option<String>,
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct AttentionTask {
    pub task_id: String,
    pub title: String,
    /// NEEDS_HUMAN atau CONFLICT.
    pub status: String,
    /// Versi saat ini; dikirim kembali sebagai `expected_version` supaya keputusan atas data basi ditolak.
    pub version: i64,
    pub run_id: Uuid,
    pub project_name: String,
    pub run_objective: String,
    pub updated_at: String,
    /// Kode error attempt terakhir (mis. recovery.tool_in_progress), bila ada.
    pub last_error: Option<String>,
    pub has_patch: bool,
    /// Hanya NEEDS_HUMAN yang boleh di-retry; CONFLICT menunggu diserahkan ke manusia atau dibatalkan.
    pub can_retry: bool,
    pub recent_events: Vec<ContextEvent>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct PlanDecision {
    pub plan_id: String,
    pub run_id: Uuid,
    pub version: i64,
    /// APPROVED atau REJECTED.
    pub outcome: String,
    pub actor_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TaskDecision {
    pub event_id: i64,
    pub task_id: String,
    pub from_status: String,
    pub to_status: String,
    pub actor_id: String,
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ApprovalsSnapshot {
    pub pending_plans: Vec<PendingPlan>,
    pub attention_tasks: Vec<AttentionTask>,
    pub plan_decisions: Vec<PlanDecision>,
    pub task_decisions: Vec<TaskDecision>,
}

fn tokens_of(tasks: &Value) -> i64 {
    tasks
        .as_array()
        .map(|tasks| {
            tasks
                .iter()
                .map(|task| {
                    task["limits"]["max_input_tokens"]
                        .as_i64()
                        .unwrap_or(0)
                        .saturating_add(task["limits"]["max_output_tokens"].as_i64().unwrap_or(0))
                })
                .fold(0_i64, i64::saturating_add)
        })
        .unwrap_or(0)
}

const ISO: &str = "to_char(%c AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')";

fn iso(column: &str) -> String {
    ISO.replace("%c", column)
}

pub async fn approvals_snapshot(pool: &PgPool) -> Result<ApprovalsSnapshot, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;

    let pending_plans = sqlx::query(&format!(
        "SELECT pl.id,pl.project_run_id,pl.version,pl.tasks,pl.risk_flags,{created} AS created_at,
                pr.objective,pr.status,pr.token_budget,pr.reserved_tokens,p.name
         FROM plans pl JOIN project_runs pr ON pr.id=pl.project_run_id JOIN projects p ON p.id=pr.project_id
         WHERE pl.status='PROPOSED' ORDER BY pl.created_at,pl.id LIMIT $1",
        created = iso("pl.created_at")
    ))
    .bind(MAX_PENDING)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| {
        let tasks: Value = row.get("tasks");
        let reserved = tokens_of(&tasks);
        let available = row.get::<i64, _>("token_budget").saturating_sub(row.get::<i64, _>("reserved_tokens")).max(0);
        PendingPlan {
            plan_id: row.get("id"),
            run_id: row.get("project_run_id"),
            project_name: row.get("name"),
            run_objective: row.get("objective"),
            run_status: row.get("status"),
            version: row.get("version"),
            task_count: tasks.as_array().map_or(0, Vec::len),
            risk_flags: serde_json::from_value(row.get("risk_flags")).unwrap_or_default(),
            reserved_tokens: reserved,
            available_tokens: available,
            over_budget: reserved > available,
            created_at: row.get("created_at"),
        }
    })
    .collect();

    let attention = sqlx::query(&format!(
        "SELECT t.id,t.title,t.status,t.version,t.project_run_id,{updated} AS updated_at,pr.objective,p.name,
                (SELECT ar.error_code FROM agent_runs ar WHERE ar.task_id=t.id ORDER BY ar.attempt DESC LIMIT 1) AS last_error,
                EXISTS(SELECT 1 FROM artifacts a WHERE a.task_id=t.id AND a.kind='patch') AS has_patch
         FROM tasks t JOIN project_runs pr ON pr.id=t.project_run_id JOIN projects p ON p.id=pr.project_id
         WHERE t.status IN ('NEEDS_HUMAN','CONFLICT') ORDER BY t.updated_at,t.id LIMIT $1",
        updated = iso("t.updated_at")
    ))
    .bind(MAX_ATTENTION)
    .fetch_all(&mut *tx)
    .await?;
    let mut attention_tasks = Vec::with_capacity(attention.len());
    for row in &attention {
        let task_id: String = row.get("id");
        let mut recent: Vec<ContextEvent> = sqlx::query(&format!(
            "SELECT id,event_type,actor_type,actor_id,from_status,to_status,payload,{at} AS at
             FROM events WHERE task_id=$1 ORDER BY id DESC LIMIT $2",
            at = iso("created_at")
        ))
        .bind(&task_id)
        .bind(EVENTS_PER_TASK)
        .fetch_all(&mut *tx)
        .await?
        .iter()
        .map(|event| ContextEvent {
            id: event.get("id"),
            event_type: event.get("event_type"),
            actor: event.get("actor_type"),
            actor_id: event.get("actor_id"),
            from_status: event.get("from_status"),
            to_status: event.get("to_status"),
            payload: event.get("payload"),
            created_at: event.get("at"),
        })
        .collect();
        recent.reverse();
        let status: String = row.get("status");
        attention_tasks.push(AttentionTask {
            task_id,
            title: row.get("title"),
            can_retry: status == "NEEDS_HUMAN",
            status,
            version: row.get("version"),
            run_id: row.get("project_run_id"),
            project_name: row.get("name"),
            run_objective: row.get("objective"),
            updated_at: row.get("updated_at"),
            last_error: row.get("last_error"),
            has_patch: row.get("has_patch"),
            recent_events: recent,
        });
    }

    let plan_decisions = sqlx::query(
        "SELECT id,project_run_id,version,status,actor_id,reason FROM plans
         WHERE status IN ('APPROVED','REJECTED','SUPERSEDED') AND actor_id IS NOT NULL
         ORDER BY created_at DESC,id LIMIT $1",
    )
    .bind(MAX_HISTORY)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| PlanDecision {
        plan_id: row.get("id"),
        run_id: row.get("project_run_id"),
        version: row.get("version"),
        // SUPERSEDED adalah plan yang dulu disetujui lalu digantikan.
        outcome: if row.get::<String, _>("status") == "REJECTED" {
            "REJECTED"
        } else {
            "APPROVED"
        }
        .to_owned(),
        actor_id: row.get("actor_id"),
        reason: row.get("reason"),
    })
    .collect();

    let task_decisions = sqlx::query(&format!(
        "SELECT id,task_id,from_status,to_status,actor_id,payload->>'reason' AS reason,{at} AS at FROM events
         WHERE actor_type='human' AND actor_id IS NOT NULL AND event_type='status_transition' AND task_id IS NOT NULL
         ORDER BY id DESC LIMIT $1",
        at = iso("created_at")
    ))
    .bind(MAX_HISTORY)
    .fetch_all(&mut *tx)
    .await?
    .iter()
    .map(|row| TaskDecision {
        event_id: row.get("id"),
        task_id: row.get("task_id"),
        from_status: row.get("from_status"),
        to_status: row.get("to_status"),
        actor_id: row.get("actor_id"),
        reason: row.get("reason"),
        created_at: row.get("at"),
    })
    .collect();

    tx.commit().await?;
    Ok(ApprovalsSnapshot {
        pending_plans,
        attention_tasks,
        plan_decisions,
        task_decisions,
    })
}
