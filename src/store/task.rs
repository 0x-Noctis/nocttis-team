use std::{error::Error, fmt};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use uuid::Uuid;

use crate::domain::{
    state_machine::{Actor, TransitionError, transition},
    task::{
        MaxAttempts, NonEmptyString, PositiveLimit, TaskContract, TaskLimits, TaskStatus,
        ValidationError,
    },
};

use super::event::{
    AgentAttempt, ArtifactRecord, AttemptStatus, AttemptUpdate, ClaimAttempt, DispatchClaim,
    NumericError, RecoveryDisposition, RecoveryResult, RuntimeAttempt, TaskEvent, ToolCallMetadata,
    ToolCallReservation, ToolOutcome, Usage,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    TaskId,
    Attempt,
    Claim,
    ToolCall,
    Usage,
    StaleVersion,
}

pub enum StoreError {
    Conflict(Conflict),
    NotFound,
    InvalidTransition(TransitionError),
    InvalidRow(ValidationError),
    InvalidNumeric(NumericError),
    InvalidId(&'static str),
    InvalidJson(serde_json::Error),
    Database(sqlx::Error),
}

impl fmt::Debug for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(conflict) => write!(formatter, "conflict: {conflict:?}"),
            Self::NotFound => formatter.write_str("resource not found"),
            Self::InvalidTransition(error) => error.fmt(formatter),
            Self::InvalidRow(error) => write!(formatter, "invalid database row: {error}"),
            Self::InvalidNumeric(error) => {
                write!(formatter, "invalid numeric field: {}", error.field)
            }
            Self::InvalidId(field) => write!(formatter, "{field} must be a UUID"),
            Self::InvalidJson(_) => formatter.write_str("invalid JSON in database row"),
            Self::Database(_) => formatter.write_str("database operation failed"),
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidTransition(error) => Some(error),
            Self::InvalidRow(error) => Some(error),
            Self::InvalidJson(error) => Some(error),
            Self::Database(_) => None,
            _ => None,
        }
    }
}

impl From<ValidationError> for StoreError {
    fn from(error: ValidationError) -> Self {
        Self::InvalidRow(error)
    }
}
impl From<TransitionError> for StoreError {
    fn from(error: TransitionError) -> Self {
        Self::InvalidTransition(error)
    }
}
impl From<NumericError> for StoreError {
    fn from(error: NumericError) -> Self {
        Self::InvalidNumeric(error)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidJson(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTask {
    pub contract: TaskContract,
    pub status: TaskStatus,
    pub version: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone)]
pub struct TaskRepository {
    pool: PgPool,
}

impl TaskRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, contract: &TaskContract) -> Result<StoredTask, StoreError> {
        contract.validate_for_status(TaskStatus::Draft)?;
        let project_id = uuid("project_id", contract.project_id.as_str())?;
        let project_run_id = uuid("project_run_id", contract.project_run_id.as_str())?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let result = sqlx::query(
            "INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,
             acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,
             max_attempts,context_refs,max_tool_calls,timeout_seconds)
             SELECT $1,pr.id,$2,$3,$4,'DRAFT',$5,$6,$7,$8,$9,$10,$11,$12,$13
             FROM project_runs pr WHERE pr.id=$14 AND pr.project_id=$15",
        )
        .bind(contract.id.as_str())
        .bind(contract.role.as_str())
        .bind(contract.title.as_str())
        .bind(contract.objective.as_str())
        .bind(serde_json::to_value(&contract.allowed_paths)?)
        .bind(serde_json::to_value(&contract.acceptance_criteria)?)
        .bind(serde_json::to_value(&contract.verification_commands)?)
        .bind(contract.limits.max_input_tokens.get())
        .bind(contract.limits.max_output_tokens.get())
        .bind(
            i16::try_from(contract.limits.max_attempts.get()).expect("max attempts is at most 10"),
        )
        .bind(serde_json::to_value(&contract.context_refs)?)
        .bind(contract.limits.max_tool_calls.get())
        .bind(contract.limits.timeout_seconds.get())
        .bind(project_run_id)
        .bind(project_id)
        .execute(&mut *transaction)
        .await;
        match result {
            Ok(result) if result.rows_affected() == 0 => return Err(StoreError::NotFound),
            Ok(_) => {}
            Err(error) if constraint(&error) == Some("tasks_pkey") => {
                return Err(StoreError::Conflict(Conflict::TaskId));
            }
            Err(error) => return Err(StoreError::Database(error)),
        }
        replace_dependencies(&mut transaction, contract).await?;
        transaction.commit().await.map_err(StoreError::Database)?;
        Ok(StoredTask {
            contract: contract.clone(),
            status: TaskStatus::Draft,
            version: 0,
        })
    }

    pub async fn get(&self, id: &str) -> Result<StoredTask, StoreError> {
        fetch_task(&self.pool, id).await
    }

    pub async fn update(
        &self,
        contract: &TaskContract,
        expected_version: i64,
    ) -> Result<StoredTask, StoreError> {
        let project_id = uuid("project_id", contract.project_id.as_str())?;
        let project_run_id = uuid("project_run_id", contract.project_run_id.as_str())?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let status: Option<String> =
            sqlx::query_scalar("SELECT status::text FROM tasks WHERE id=$1 FOR UPDATE")
                .bind(contract.id.as_str())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(StoreError::Database)?;
        let status = status.ok_or(StoreError::NotFound).and_then(parse_enum)?;
        contract.validate_for_status(status)?;
        let result = sqlx::query(
            "UPDATE tasks t SET project_run_id=pr.id,role=$2,title=$3,objective=$4,allowed_paths=$5,
             acceptance_criteria=$6,verification_commands=$7,max_input_tokens=$8,max_output_tokens=$9,
             max_attempts=$10,context_refs=$11,max_tool_calls=$12,timeout_seconds=$13,
             version=version+1,updated_at=now() FROM project_runs pr
             WHERE t.id=$1 AND t.version=$14 AND pr.id=$15 AND pr.project_id=$16",
        )
        .bind(contract.id.as_str()).bind(contract.role.as_str()).bind(contract.title.as_str())
        .bind(contract.objective.as_str()).bind(serde_json::to_value(&contract.allowed_paths)?)
        .bind(serde_json::to_value(&contract.acceptance_criteria)?)
        .bind(serde_json::to_value(&contract.verification_commands)?)
        .bind(contract.limits.max_input_tokens.get()).bind(contract.limits.max_output_tokens.get())
        .bind(i16::try_from(contract.limits.max_attempts.get()).expect("max attempts is at most 10"))
        .bind(serde_json::to_value(&contract.context_refs)?)
        .bind(contract.limits.max_tool_calls.get()).bind(contract.limits.timeout_seconds.get())
        .bind(expected_version).bind(project_run_id).bind(project_id)
        .execute(&mut *transaction).await.map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict(Conflict::StaleVersion));
        }
        replace_dependencies(&mut transaction, contract).await?;
        transaction.commit().await.map_err(StoreError::Database)?;
        self.get(contract.id.as_str()).await
    }

    pub async fn delete(&self, id: &str, expected_version: i64) -> Result<(), StoreError> {
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let version =
            sqlx::query_scalar::<_, i64>("SELECT version FROM tasks WHERE id=$1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(StoreError::Database)?
                .ok_or(StoreError::NotFound)?;
        if version != expected_version {
            return Err(StoreError::Conflict(Conflict::StaleVersion));
        }
        let result = sqlx::query("DELETE FROM tasks WHERE id=$1 AND version=$2")
            .bind(id.to_owned())
            .bind(expected_version)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict(Conflict::StaleVersion));
        }
        transaction.commit().await.map_err(StoreError::Database)?;
        Ok(())
    }

    pub async fn list(
        &self,
        cursor: Option<&str>,
        limit: i64,
    ) -> Result<Page<StoredTask>, StoreError> {
        if limit <= 0 {
            return Err(StoreError::InvalidRow(ValidationError {
                field: "limit",
                message: "must be positive",
            }));
        }
        let limit = limit.min(100);
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM tasks WHERE ($1::text IS NULL OR id>$1) ORDER BY id LIMIT $2",
        )
        .bind(cursor)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        let has_more = ids.len() > limit as usize;
        let visible = &ids[..ids.len().min(limit as usize)];
        let mut items = Vec::with_capacity(visible.len());
        for id in visible {
            items.push(self.get(id).await?);
        }
        Ok(Page {
            items,
            next_cursor: has_more.then(|| visible.last().expect("page has item").clone()),
        })
    }

    pub async fn transition(
        &self,
        id: &str,
        expected_version: i64,
        to: TaskStatus,
        actor: Actor,
    ) -> Result<StoredTask, StoreError> {
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = task_query("WHERE t.id=$1 FOR UPDATE")
            .bind(id.to_owned())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        let current = task_from_row(&row)?;
        if current.version != expected_version {
            return Err(StoreError::Conflict(Conflict::StaleVersion));
        }
        transition(&current.contract, current.status, to, actor)?;
        sqlx::query("UPDATE tasks SET status=$2,version=version+1,updated_at=now() WHERE id=$1 AND version=$3")
            .bind(id).bind(enum_text(&to)?).bind(expected_version).execute(&mut *transaction).await.map_err(StoreError::Database)?;
        insert_event(
            &mut transaction,
            &current.contract,
            actor,
            current.status,
            to,
        )
        .await?;
        transaction.commit().await.map_err(StoreError::Database)?;
        Ok(StoredTask {
            contract: current.contract,
            status: to,
            version: expected_version + 1,
        })
    }

    pub async fn events(&self, task_id: &str) -> Result<Vec<TaskEvent>, StoreError> {
        let rows = sqlx::query("SELECT id,task_id,actor_type,event_type,from_status::text,to_status::text,payload FROM events WHERE task_id=$1 ORDER BY id")
            .bind(task_id).fetch_all(&self.pool).await.map_err(StoreError::Database)?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn events_after(
        &self,
        task_id: &str,
        cursor: i64,
    ) -> Result<Vec<TaskEvent>, StoreError> {
        let rows = sqlx::query("SELECT id,task_id,actor_type,event_type,from_status::text,to_status::text,payload FROM events WHERE task_id=$1 AND id>$2 ORDER BY id LIMIT 100")
            .bind(task_id).bind(cursor).fetch_all(&self.pool).await.map_err(StoreError::Database)?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn create_attempt(&self, attempt: &AgentAttempt) -> Result<(), StoreError> {
        attempt.validate()?;
        let result = sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(attempt.id).bind(attempt.task_id.as_str()).bind(attempt.role.as_str())
            .bind(attempt.provider_id.as_str()).bind(attempt.model_id.as_str()).bind(attempt.attempt)
            .bind(attempt.status.as_str()).execute(&self.pool).await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if constraint(&error) == Some("agent_runs_task_id_attempt_key") => {
                Err(StoreError::Conflict(Conflict::Attempt))
            }
            Err(error) => Err(StoreError::Database(error)),
        }
    }

    pub async fn create_runtime_attempt(&self, attempt: &RuntimeAttempt) -> Result<(), StoreError> {
        validate_runtime_attempt(attempt)?;
        let result = sqlx::query(
            "INSERT INTO agent_runs
             (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,
              heartbeat_at,finished_at,retain_until,error_code)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,
              CASE WHEN $10::bigint IS NULL THEN NULL ELSE to_timestamp($10::double precision/1000) END,
              CASE WHEN $11::bigint IS NULL THEN NULL ELSE to_timestamp($11::double precision/1000) END,
              CASE WHEN $12::bigint IS NULL THEN NULL ELSE to_timestamp($12::double precision/1000) END,$13)",
        )
        .bind(attempt.id)
        .bind(attempt.task_id.as_str())
        .bind(attempt.role.as_str())
        .bind(attempt.provider_id.as_str())
        .bind(attempt.model_id.as_str())
        .bind(attempt.attempt)
        .bind(attempt.status.as_str())
        .bind(&attempt.branch)
        .bind(&attempt.base_commit)
        .bind(attempt.heartbeat_unix_ms)
        .bind(attempt.finished_unix_ms)
        .bind(attempt.retain_until_unix_ms)
        .bind(&attempt.error_code)
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if constraint(&error) == Some("agent_runs_task_id_attempt_key") => {
                Err(StoreError::Conflict(Conflict::Attempt))
            }
            Err(error) => Err(StoreError::Database(error)),
        }
    }

    pub async fn get_attempt(&self, id: Uuid) -> Result<RuntimeAttempt, StoreError> {
        let row = attempt_query("WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        runtime_attempt_from_row(&row)
    }

    pub async fn list_attempts(&self, task_id: &str) -> Result<Vec<RuntimeAttempt>, StoreError> {
        let rows = attempt_query(
            "WHERE task_id=$1 AND branch IS NOT NULL AND base_commit IS NOT NULL ORDER BY attempt,id",
        )
            .bind(task_id.to_owned())
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.iter().map(runtime_attempt_from_row).collect()
    }

    pub async fn unfinished_attempts(&self) -> Result<Vec<RuntimeAttempt>, StoreError> {
        let rows = attempt_query(
            "WHERE finished_at IS NULL AND branch IS NOT NULL AND base_commit IS NOT NULL ORDER BY heartbeat_at NULLS FIRST,started_at,id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        rows.iter().map(runtime_attempt_from_row).collect()
    }

    pub async fn update_attempt(
        &self,
        id: Uuid,
        update: &AttemptUpdate,
    ) -> Result<RuntimeAttempt, StoreError> {
        validate_error_code(update.error_code.as_deref())?;
        let terminal = matches!(
            update.status,
            AttemptStatus::Completed | AttemptStatus::Failed
        );
        let result = sqlx::query(
            "UPDATE agent_runs SET status=$2,error_code=$3,heartbeat_at=now(),
             finished_at=CASE WHEN $4 THEN COALESCE(finished_at,now()) ELSE NULL END
             WHERE id=$1",
        )
        .bind(id)
        .bind(update.status.as_str())
        .bind(&update.error_code)
        .bind(terminal)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        self.get_attempt(id).await
    }

    pub async fn claim_ready(
        &self,
        expected_version: i64,
        claim: &ClaimAttempt,
    ) -> Result<RuntimeAttempt, StoreError> {
        validate_claim(claim)?;
        if !(0..=crate::domain::task::MAX_SAFE_INTEGER).contains(&expected_version) {
            return Err(invalid_numeric("expected_version"));
        }
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = sqlx::query(
            "SELECT status,version,max_attempts,project_run_id FROM tasks WHERE id=$1 FOR UPDATE",
        )
        .bind(claim.task_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(StoreError::Database)?
        .ok_or(StoreError::NotFound)?;
        let status: String = row.get("status");
        let version: i64 = row.get("version");
        if status != "READY" || version != expected_version {
            return Err(StoreError::Conflict(Conflict::Claim));
        }
        let max_attempts = i64::from(row.get::<i16, _>("max_attempts"));
        let attempt: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt),0)+1 FROM agent_runs WHERE task_id=$1",
        )
        .bind(claim.task_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        if attempt > max_attempts {
            return Err(StoreError::Conflict(Conflict::Attempt));
        }
        sqlx::query("UPDATE tasks SET status='ASSIGNED',version=version+1,updated_at=now() WHERE id=$1 AND version=$2 AND status='READY'")
            .bind(claim.task_id.as_str()).bind(expected_version).execute(&mut *transaction).await.map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,from_status,to_status,payload) VALUES ($1,$2,'system','status_transition','READY','ASSIGNED','{}')")
            .bind(row.get::<Uuid,_>("project_run_id")).bind(claim.task_id.as_str()).execute(&mut *transaction).await.map_err(StoreError::Database)?;
        sqlx::query(
            "INSERT INTO agent_runs
             (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until)
             VALUES ($1,$2,$3,$4,$5,$6,'assigned',$7,$8,now(),now()+($9 * interval '1 second'))",
        )
        .bind(claim.id).bind(claim.task_id.as_str()).bind(claim.role.as_str())
        .bind(claim.provider_id.as_str()).bind(claim.model_id.as_str()).bind(attempt)
        .bind(&claim.branch).bind(&claim.base_commit).bind(claim.retention_seconds)
        .execute(&mut *transaction).await.map_err(StoreError::Database)?;
        transaction.commit().await.map_err(StoreError::Database)?;
        self.get_attempt(claim.id).await
    }

    pub async fn claim_next_ready(
        &self,
        claim: &DispatchClaim,
    ) -> Result<Option<RuntimeAttempt>, StoreError> {
        validate_dispatch_claim(claim)?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let row = sqlx::query(
            "SELECT id,version,max_attempts,project_run_id FROM tasks
             WHERE status='READY'
               AND (SELECT count(*) FROM agent_runs WHERE task_id=tasks.id) < max_attempts
             ORDER BY priority DESC,created_at,id
             FOR UPDATE SKIP LOCKED LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        let Some(row) = row else {
            transaction.commit().await.map_err(StoreError::Database)?;
            return Ok(None);
        };
        let task_id = row.get::<String, _>("id");
        let attempt: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(attempt),0)+1 FROM agent_runs WHERE task_id=$1",
        )
        .bind(&task_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        if attempt > i64::from(row.get::<i16, _>("max_attempts")) {
            return Err(StoreError::Conflict(Conflict::Attempt));
        }
        sqlx::query(
            "UPDATE tasks SET status='ASSIGNED',version=version+1,updated_at=now() WHERE id=$1",
        )
        .bind(&task_id)
        .execute(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,from_status,to_status,payload) VALUES ($1,$2,'system','status_transition','READY','ASSIGNED','{}')")
            .bind(row.get::<Uuid,_>("project_run_id")).bind(&task_id).execute(&mut *transaction).await.map_err(StoreError::Database)?;
        sqlx::query("INSERT INTO agent_runs (id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,heartbeat_at,retain_until) VALUES ($1,$2,$3,$4,$5,$6,'assigned',$7,$8,now(),now()+($9 * interval '1 second'))")
            .bind(claim.id).bind(&task_id).bind(claim.role.as_str()).bind(claim.provider_id.as_str())
            .bind(claim.model_id.as_str()).bind(attempt).bind(&claim.branch).bind(&claim.base_commit)
            .bind(claim.retention_seconds).execute(&mut *transaction).await.map_err(StoreError::Database)?;
        transaction.commit().await.map_err(StoreError::Database)?;
        self.get_attempt(claim.id).await.map(Some)
    }

    pub async fn start_claimed(&self, attempt_id: Uuid) -> Result<RuntimeAttempt, StoreError> {
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let attempt = sqlx::query("SELECT task_id,status FROM agent_runs WHERE id=$1 FOR UPDATE")
            .bind(attempt_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        let task_id = attempt.get::<String, _>("task_id");
        let task = sqlx::query("SELECT status,project_run_id FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(&task_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        if attempt.get::<String, _>("status") != "assigned"
            || task.get::<String, _>("status") != "ASSIGNED"
        {
            return Err(StoreError::Conflict(Conflict::Claim));
        }
        sqlx::query("UPDATE agent_runs SET status='running',heartbeat_at=now() WHERE id=$1")
            .bind(attempt_id)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        sqlx::query(
            "UPDATE tasks SET status='RUNNING',version=version+1,updated_at=now() WHERE id=$1",
        )
        .bind(&task_id)
        .execute(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        insert_transition(
            &mut transaction,
            task.get("project_run_id"),
            &task_id,
            "ASSIGNED",
            "RUNNING",
        )
        .await?;
        transaction.commit().await.map_err(StoreError::Database)?;
        self.get_attempt(attempt_id).await
    }

    pub async fn recover_stale(
        &self,
        heartbeat_before_unix_ms: i64,
    ) -> Result<Option<RecoveryResult>, StoreError> {
        non_negative_safe(heartbeat_before_unix_ms, "heartbeat cutoff")?;
        let mut transaction = self.pool.begin().await.map_err(StoreError::Database)?;
        let attempt = sqlx::query("SELECT id,task_id,status FROM agent_runs WHERE finished_at IS NULL AND status IN ('assigned','running') AND COALESCE(heartbeat_at,started_at) < to_timestamp($1::double precision/1000) ORDER BY COALESCE(heartbeat_at,started_at),id FOR UPDATE SKIP LOCKED LIMIT 1")
            .bind(heartbeat_before_unix_ms).fetch_optional(&mut *transaction).await.map_err(StoreError::Database)?;
        let Some(attempt) = attempt else {
            transaction.commit().await.map_err(StoreError::Database)?;
            return Ok(None);
        };
        let attempt_id = attempt.get::<Uuid, _>("id");
        let task_id = attempt.get::<String, _>("task_id");
        let task = sqlx::query("SELECT status,project_run_id FROM tasks WHERE id=$1 FOR UPDATE")
            .bind(&task_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
        let from = task.get::<String, _>("status");
        let ambiguous: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tool_call_reservations WHERE agent_run_id=$1 AND status='in_progress')")
            .bind(attempt_id).fetch_one(&mut *transaction).await.map_err(StoreError::Database)?;
        let disposition = if ambiguous {
            sqlx::query("UPDATE agent_runs SET status='recovery_required',error_code='recovery.tool_in_progress',heartbeat_at=now(),finished_at=now() WHERE id=$1")
                .bind(attempt_id).execute(&mut *transaction).await.map_err(StoreError::Database)?;
            RecoveryDisposition::RecoveryRequired
        } else {
            sqlx::query("UPDATE agent_runs SET status='failed',error_code='recovery.stale',heartbeat_at=now(),finished_at=now() WHERE id=$1")
                .bind(attempt_id).execute(&mut *transaction).await.map_err(StoreError::Database)?;
            sqlx::query(
                "UPDATE tasks SET status='READY',version=version+2,updated_at=now() WHERE id=$1",
            )
            .bind(&task_id)
            .execute(&mut *transaction)
            .await
            .map_err(StoreError::Database)?;
            insert_transition(
                &mut transaction,
                task.get("project_run_id"),
                &task_id,
                &from,
                "FAILED",
            )
            .await?;
            insert_transition(
                &mut transaction,
                task.get("project_run_id"),
                &task_id,
                "FAILED",
                "READY",
            )
            .await?;
            RecoveryDisposition::Requeued
        };
        transaction.commit().await.map_err(StoreError::Database)?;
        Ok(Some(RecoveryResult {
            attempt_id,
            task_id: NonEmptyString::parse("task_id", task_id)?,
            disposition,
        }))
    }

    pub async fn retention_due(&self) -> Result<Vec<RuntimeAttempt>, StoreError> {
        let rows = attempt_query("WHERE finished_at IS NOT NULL AND retain_until <= now() AND cleanup_completed_at IS NULL ORDER BY retain_until,id")
            .fetch_all(&self.pool).await.map_err(StoreError::Database)?;
        rows.iter().map(runtime_attempt_from_row).collect()
    }

    pub async fn complete_retention_cleanup(&self, attempt_id: Uuid) -> Result<bool, StoreError> {
        let result = sqlx::query("UPDATE agent_runs SET cleanup_completed_at=now() WHERE id=$1 AND finished_at IS NOT NULL AND cleanup_completed_at IS NULL")
            .bind(attempt_id).execute(&self.pool).await.map_err(StoreError::Database)?;
        if result.rows_affected() == 1 {
            return Ok(true);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE id=$1 AND finished_at IS NOT NULL)",
        )
        .bind(attempt_id)
        .fetch_one(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if exists {
            Ok(false)
        } else {
            Err(StoreError::NotFound)
        }
    }

    pub async fn reserve_tool_call(
        &self,
        agent_run_id: Uuid,
        call_id: &str,
    ) -> Result<ToolCallReservation, StoreError> {
        validate_key(call_id, "call ID")?;
        let inserted = sqlx::query("INSERT INTO tool_call_reservations (agent_run_id,call_id) VALUES ($1,$2) ON CONFLICT DO NOTHING")
            .bind(agent_run_id).bind(call_id).execute(&self.pool).await.map_err(StoreError::Database)?;
        if inserted.rows_affected() == 1 {
            return Ok(ToolCallReservation::New);
        }
        self.tool_call(agent_run_id, call_id).await
    }

    pub async fn complete_tool_call(
        &self,
        agent_run_id: Uuid,
        call_id: &str,
        metadata: &ToolCallMetadata,
    ) -> Result<ToolCallReservation, StoreError> {
        validate_key(call_id, "call ID")?;
        non_negative_safe(metadata.duration_ms, "duration")?;
        let result = sqlx::query(
            "UPDATE tool_call_reservations SET status='completed',outcome=$3,duration_ms=$4,
             artifact_id=$5,completed_at=now() WHERE agent_run_id=$1 AND call_id=$2 AND status='in_progress'",
        )
        .bind(agent_run_id).bind(call_id).bind(metadata.outcome.as_str())
        .bind(metadata.duration_ms).bind(metadata.artifact_id).execute(&self.pool).await.map_err(StoreError::Database)?;
        if result.rows_affected() == 1 {
            return Ok(ToolCallReservation::Completed(metadata.clone()));
        }
        match self.tool_call(agent_run_id, call_id).await? {
            ToolCallReservation::Completed(existing) if existing == *metadata => {
                Ok(ToolCallReservation::Completed(existing))
            }
            ToolCallReservation::Completed(_) => Err(StoreError::Conflict(Conflict::ToolCall)),
            ToolCallReservation::InProgress | ToolCallReservation::New => {
                Err(StoreError::Conflict(Conflict::ToolCall))
            }
        }
    }

    async fn tool_call(
        &self,
        agent_run_id: Uuid,
        call_id: &str,
    ) -> Result<ToolCallReservation, StoreError> {
        let row = sqlx::query("SELECT status,outcome,duration_ms,artifact_id FROM tool_call_reservations WHERE agent_run_id=$1 AND call_id=$2")
            .bind(agent_run_id).bind(call_id).fetch_optional(&self.pool).await.map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        if row.get::<String, _>("status") == "in_progress" {
            return Ok(ToolCallReservation::InProgress);
        }
        let outcome = ToolOutcome::parse(&row.get::<String, _>("outcome"))
            .ok_or_else(|| invalid_numeric("tool_outcome"))?;
        Ok(ToolCallReservation::Completed(ToolCallMetadata {
            outcome,
            duration_ms: row.get("duration_ms"),
            artifact_id: row.get("artifact_id"),
        }))
    }

    pub async fn record_usage(&self, attempt_id: Uuid, usage: &Usage) -> Result<i64, StoreError> {
        usage.validate()?;
        sqlx::query_scalar("INSERT INTO model_usage (agent_run_id,input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(attempt_id).bind(usage.input_tokens).bind(usage.cached_tokens).bind(usage.output_tokens)
            .bind(usage.tool_calls).bind(usage.latency_ms).bind(usage.estimated)
            .fetch_one(&self.pool).await.map_err(StoreError::Database)
    }

    pub async fn record_usage_once(
        &self,
        attempt_id: Uuid,
        operation_key: &str,
        usage: &Usage,
    ) -> Result<i64, StoreError> {
        validate_key(operation_key, "operation key")?;
        usage.validate()?;
        let inserted = sqlx::query_scalar(
            "INSERT INTO model_usage
             (agent_run_id,operation_key,input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (agent_run_id,operation_key) WHERE operation_key IS NOT NULL
             DO NOTHING RETURNING id",
        )
        .bind(attempt_id)
        .bind(operation_key)
        .bind(usage.input_tokens)
        .bind(usage.cached_tokens)
        .bind(usage.output_tokens)
        .bind(usage.tool_calls)
        .bind(usage.latency_ms)
        .bind(usage.estimated)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if let Some(id) = inserted {
            return Ok(id);
        }
        let row = sqlx::query("SELECT id,input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated FROM model_usage WHERE agent_run_id=$1 AND operation_key=$2")
            .bind(attempt_id).bind(operation_key).fetch_one(&self.pool).await.map_err(StoreError::Database)?;
        let matches = row.get::<i64, _>("input_tokens") == usage.input_tokens
            && row.get::<i64, _>("cached_tokens") == usage.cached_tokens
            && row.get::<i64, _>("output_tokens") == usage.output_tokens
            && row.get::<i64, _>("tool_calls") == usage.tool_calls
            && row.get::<i64, _>("latency_ms") == usage.latency_ms
            && row.get::<bool, _>("estimated") == usage.estimated;
        if !matches {
            return Err(StoreError::Conflict(Conflict::Usage));
        }
        Ok(row.get("id"))
    }

    pub async fn persist_artifact(&self, artifact: &ArtifactRecord) -> Result<(), StoreError> {
        validate_artifact(artifact)?;
        sqlx::query(
            "INSERT INTO artifacts
             (id,task_id,kind,path,logical_name,media_type,size_bytes,sha256)
             VALUES ($1,$2,$3,NULL,$4,$5,$6,$7)",
        )
        .bind(artifact.id)
        .bind(artifact.task_id.as_str())
        .bind(artifact.kind.as_str())
        .bind(artifact.logical_name.as_str())
        .bind(artifact.media_type.as_str())
        .bind(artifact.size)
        .bind(&artifact.checksum)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        Ok(())
    }

    pub async fn delete_artifact(&self, id: Uuid) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM artifacts WHERE id=$1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        Ok(())
    }

    pub async fn usage(&self, id: i64) -> Result<Usage, StoreError> {
        let row = sqlx::query("SELECT input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated FROM model_usage WHERE id=$1")
            .bind(id).fetch_optional(&self.pool).await.map_err(StoreError::Database)?.ok_or(StoreError::NotFound)?;
        Ok(Usage {
            input_tokens: row.get("input_tokens"),
            cached_tokens: row.get("cached_tokens"),
            output_tokens: row.get("output_tokens"),
            tool_calls: row.get("tool_calls"),
            latency_ms: row.get("latency_ms"),
            estimated: row.get("estimated"),
        })
    }
}

async fn fetch_task(pool: &PgPool, id: &str) -> Result<StoredTask, StoreError> {
    let row = task_query("WHERE t.id=$1")
        .bind(id.to_owned())
        .fetch_optional(pool)
        .await
        .map_err(StoreError::Database)?
        .ok_or(StoreError::NotFound)?;
    task_from_row(&row)
}

fn task_query(suffix: &str) -> sqlx::query::Query<'static, Postgres, sqlx::postgres::PgArguments> {
    let sql = format!(
        "SELECT t.*,pr.project_id::text AS project_id,COALESCE((SELECT jsonb_agg(td.dependency_id ORDER BY td.dependency_id) FROM task_dependencies td WHERE td.task_id=t.id),'[]') AS depends_on FROM tasks t JOIN project_runs pr ON pr.id=t.project_run_id {suffix}"
    );
    sqlx::query(Box::leak(sql.into_boxed_str()))
}

fn attempt_query(
    suffix: &str,
) -> sqlx::query::Query<'static, Postgres, sqlx::postgres::PgArguments> {
    let sql = format!(
        "SELECT id,task_id,role,provider_id,model_id,attempt,status,branch,base_commit,
         (extract(epoch FROM heartbeat_at)*1000)::bigint AS heartbeat_unix_ms,
         (extract(epoch FROM finished_at)*1000)::bigint AS finished_unix_ms,
         (extract(epoch FROM retain_until)*1000)::bigint AS retain_until_unix_ms,error_code
         FROM agent_runs {suffix}"
    );
    sqlx::query(Box::leak(sql.into_boxed_str()))
}

fn runtime_attempt_from_row(row: &PgRow) -> Result<RuntimeAttempt, StoreError> {
    let status = row.get::<String, _>("status");
    Ok(RuntimeAttempt {
        id: row.get("id"),
        task_id: text(row, "task_id", "task_id")?,
        role: text(row, "role", "role")?,
        provider_id: text(row, "provider_id", "provider_id")?,
        model_id: text(row, "model_id", "model_id")?,
        attempt: row.get("attempt"),
        status: AttemptStatus::parse(&status).ok_or_else(|| invalid_numeric("attempt_status"))?,
        branch: row.get("branch"),
        base_commit: row.get("base_commit"),
        heartbeat_unix_ms: row.get("heartbeat_unix_ms"),
        finished_unix_ms: row.get("finished_unix_ms"),
        retain_until_unix_ms: row.get("retain_until_unix_ms"),
        error_code: row.get("error_code"),
    })
}

fn validate_runtime_attempt(attempt: &RuntimeAttempt) -> Result<(), StoreError> {
    if !(1..=crate::domain::task::MAX_SAFE_INTEGER).contains(&attempt.attempt) {
        return Err(invalid_numeric("attempt"));
    }
    validate_component(&attempt.branch, "branch")?;
    validate_base_commit(&attempt.base_commit)?;
    validate_error_code(attempt.error_code.as_deref())?;
    for value in [
        attempt.heartbeat_unix_ms,
        attempt.finished_unix_ms,
        attempt.retain_until_unix_ms,
    ]
    .into_iter()
    .flatten()
    {
        non_negative_safe(value, "timestamp")?;
    }
    Ok(())
}

fn validate_claim(claim: &ClaimAttempt) -> Result<(), StoreError> {
    validate_component(&claim.branch, "branch")?;
    validate_base_commit(&claim.base_commit)?;
    if !(1..=crate::domain::task::MAX_SAFE_INTEGER).contains(&claim.retention_seconds) {
        return Err(invalid_numeric("retention"));
    }
    Ok(())
}

fn validate_dispatch_claim(claim: &DispatchClaim) -> Result<(), StoreError> {
    validate_component(&claim.branch, "branch")?;
    validate_base_commit(&claim.base_commit)?;
    if !(1..=crate::domain::task::MAX_SAFE_INTEGER).contains(&claim.retention_seconds) {
        return Err(invalid_numeric("retention"));
    }
    Ok(())
}

async fn insert_transition(
    transaction: &mut Transaction<'_, Postgres>,
    project_run_id: Uuid,
    task_id: &str,
    from: &str,
    to: &str,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,from_status,to_status,payload) VALUES ($1,$2,'system','status_transition',$3,$4,'{}')")
        .bind(project_run_id).bind(task_id).bind(from).bind(to)
        .execute(&mut **transaction).await.map_err(StoreError::Database)?;
    Ok(())
}

fn validate_artifact(artifact: &ArtifactRecord) -> Result<(), StoreError> {
    non_negative_safe(artifact.size, "artifact size")?;
    validate_component(artifact.logical_name.as_str(), "logical name")?;
    if artifact.media_type.as_str().chars().any(char::is_control) {
        return Err(StoreError::InvalidId("media_type"));
    }
    if artifact.checksum.len() != 64
        || !artifact
            .checksum
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StoreError::InvalidId("artifact_checksum"));
    }
    Ok(())
}

fn validate_key(value: &str, field: &'static str) -> Result<(), StoreError> {
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(StoreError::InvalidId(field));
    }
    Ok(())
}

fn validate_component(value: &str, field: &'static str) -> Result<(), StoreError> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control)
    {
        return Err(StoreError::InvalidId(field));
    }
    Ok(())
}

fn validate_error_code(value: Option<&str>) -> Result<(), StoreError> {
    if value.is_some_and(|value| {
        value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    }) {
        return Err(StoreError::InvalidId("error_code"));
    }
    Ok(())
}

fn validate_base_commit(value: &str) -> Result<(), StoreError> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StoreError::InvalidId("base_commit"));
    }
    Ok(())
}

fn non_negative_safe(value: i64, field: &'static str) -> Result<(), StoreError> {
    if !(0..=crate::domain::task::MAX_SAFE_INTEGER).contains(&value) {
        return Err(invalid_numeric(field));
    }
    Ok(())
}

fn invalid_numeric(field: &'static str) -> StoreError {
    StoreError::InvalidNumeric(NumericError { field })
}

async fn replace_dependencies(
    transaction: &mut Transaction<'_, Postgres>,
    contract: &TaskContract,
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM task_dependencies WHERE task_id=$1")
        .bind(contract.id.as_str())
        .execute(&mut **transaction)
        .await
        .map_err(StoreError::Database)?;
    for dependency in &contract.depends_on {
        sqlx::query("INSERT INTO task_dependencies (task_id,dependency_id) VALUES ($1,$2)")
            .bind(contract.id.as_str())
            .bind(dependency.as_str())
            .execute(&mut **transaction)
            .await
            .map_err(StoreError::Database)?;
    }
    Ok(())
}

async fn insert_event(
    transaction: &mut Transaction<'_, Postgres>,
    contract: &TaskContract,
    actor: Actor,
    from: TaskStatus,
    to: TaskStatus,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO events (project_run_id,task_id,actor_type,event_type,from_status,to_status,payload) VALUES ($1,$2,$3,'status_transition',$4,$5,'{}')")
        .bind(uuid("project_run_id", contract.project_run_id.as_str())?).bind(contract.id.as_str())
        .bind(enum_text(&actor)?).bind(enum_text(&from)?).bind(enum_text(&to)?)
        .execute(&mut **transaction).await.map_err(StoreError::Database)?;
    Ok(())
}

fn task_from_row(row: &PgRow) -> Result<StoredTask, StoreError> {
    let contract = TaskContract {
        id: text(row, "id", "id")?,
        project_id: text(row, "project_id", "project_id")?,
        project_run_id: NonEmptyString::parse(
            "project_run_id",
            row.try_get::<Uuid, _>("project_run_id")
                .map_err(StoreError::Database)?
                .to_string(),
        )?,
        title: text(row, "title", "title")?,
        role: text(row, "role", "role")?,
        objective: text(row, "objective", "objective")?,
        depends_on: json(row, "depends_on")?,
        allowed_paths: json(row, "allowed_paths")?,
        context_refs: json(row, "context_refs")?,
        acceptance_criteria: json(row, "acceptance_criteria")?,
        verification_commands: json(row, "verification_commands")?,
        limits: TaskLimits {
            max_input_tokens: PositiveLimit::new(
                "max_input_tokens",
                row.try_get("max_input_tokens")
                    .map_err(StoreError::Database)?,
            )?,
            max_output_tokens: PositiveLimit::new(
                "max_output_tokens",
                row.try_get("max_output_tokens")
                    .map_err(StoreError::Database)?,
            )?,
            max_tool_calls: PositiveLimit::new(
                "max_tool_calls",
                row.try_get("max_tool_calls")
                    .map_err(StoreError::Database)?,
            )?,
            max_attempts: MaxAttempts::new(i64::from(
                row.try_get::<i16, _>("max_attempts")
                    .map_err(StoreError::Database)?,
            ))?,
            timeout_seconds: PositiveLimit::new(
                "timeout_seconds",
                row.try_get("timeout_seconds")
                    .map_err(StoreError::Database)?,
            )?,
        },
    };
    let status = parse_enum(
        row.try_get::<String, _>("status")
            .map_err(StoreError::Database)?,
    )?;
    contract.validate_for_status(status)?;
    Ok(StoredTask {
        contract,
        status,
        version: row.try_get("version").map_err(StoreError::Database)?,
    })
}

fn event_from_row(row: &PgRow) -> Result<TaskEvent, StoreError> {
    Ok(TaskEvent {
        id: row.get("id"),
        task_id: text(row, "task_id", "task_id")?,
        actor: parse_enum(row.get("actor_type"))?,
        event_type: text(row, "event_type", "event_type")?,
        from_status: row
            .get::<Option<String>, _>("from_status")
            .map(parse_enum)
            .transpose()?,
        to_status: row
            .get::<Option<String>, _>("to_status")
            .map(parse_enum)
            .transpose()?,
        payload: row.get("payload"),
    })
}

fn text(row: &PgRow, column: &str, field: &'static str) -> Result<NonEmptyString, StoreError> {
    NonEmptyString::parse(
        field,
        row.try_get::<String, _>(column)
            .map_err(StoreError::Database)?,
    )
    .map_err(Into::into)
}
fn json<T: DeserializeOwned>(row: &PgRow, column: &str) -> Result<T, StoreError> {
    Ok(serde_json::from_value(
        row.try_get::<Value, _>(column)
            .map_err(StoreError::Database)?,
    )?)
}
fn enum_text(value: &impl Serialize) -> Result<String, StoreError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            StoreError::InvalidJson(serde_json::Error::io(std::io::Error::other(
                "enum must serialize as string",
            )))
        })
}
fn parse_enum<T: DeserializeOwned>(value: String) -> Result<T, StoreError> {
    Ok(serde_json::from_value(Value::String(value))?)
}
fn uuid(field: &'static str, value: &str) -> Result<Uuid, StoreError> {
    let parsed = Uuid::parse_str(value).map_err(|_| StoreError::InvalidId(field))?;
    if parsed.to_string() != value {
        return Err(StoreError::InvalidId(field));
    }
    Ok(parsed)
}
fn constraint(error: &sqlx::Error) -> Option<&str> {
    error
        .as_database_error()
        .and_then(|error| error.constraint())
}
