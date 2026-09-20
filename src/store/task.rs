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

use super::event::{AgentAttempt, NumericError, TaskEvent, Usage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    TaskId,
    Attempt,
    StaleVersion,
}

#[derive(Debug)]
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
            Self::Database(error) => Some(error),
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

    pub async fn delete(&self, id: &str) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM tasks WHERE id=$1")
            .bind(id.to_owned())
            .execute(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
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

    pub async fn record_usage(&self, attempt_id: Uuid, usage: &Usage) -> Result<i64, StoreError> {
        usage.validate()?;
        sqlx::query_scalar("INSERT INTO model_usage (agent_run_id,input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING id")
            .bind(attempt_id).bind(usage.input_tokens).bind(usage.cached_tokens).bind(usage.output_tokens)
            .bind(usage.tool_calls).bind(usage.latency_ms).bind(usage.estimated)
            .fetch_one(&self.pool).await.map_err(StoreError::Database)
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
