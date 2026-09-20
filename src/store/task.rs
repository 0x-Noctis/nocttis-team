use std::{error::Error, fmt};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};

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
    StaleTransition,
}

#[derive(Debug)]
pub enum StoreError {
    Conflict(Conflict),
    NotFound,
    InvalidTransition(TransitionError),
    InvalidRow(ValidationError),
    InvalidNumeric(NumericError),
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
        let result = sqlx::query(
            "INSERT INTO runtime_tasks
             (id, project_id, title, role, objective, status, depends_on, allowed_paths,
              context_refs, acceptance_criteria, verification_commands, max_input_tokens,
              max_output_tokens, max_tool_calls, max_attempts, timeout_seconds)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
        )
        .bind(contract.id.as_str())
        .bind(contract.project_id.as_str())
        .bind(contract.title.as_str())
        .bind(contract.role.as_str())
        .bind(contract.objective.as_str())
        .bind(enum_text(&TaskStatus::Draft)?)
        .bind(serde_json::to_value(&contract.depends_on)?)
        .bind(serde_json::to_value(&contract.allowed_paths)?)
        .bind(serde_json::to_value(&contract.context_refs)?)
        .bind(serde_json::to_value(&contract.acceptance_criteria)?)
        .bind(serde_json::to_value(&contract.verification_commands)?)
        .bind(contract.limits.max_input_tokens.get())
        .bind(contract.limits.max_output_tokens.get())
        .bind(contract.limits.max_tool_calls.get())
        .bind(contract.limits.max_attempts.get())
        .bind(contract.limits.timeout_seconds.get())
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => Ok(StoredTask {
                contract: contract.clone(),
                status: TaskStatus::Draft,
                version: 0,
            }),
            Err(error) if constraint(&error) == Some("runtime_tasks_pkey") => {
                Err(StoreError::Conflict(Conflict::TaskId))
            }
            Err(error) => Err(StoreError::Database(error)),
        }
    }

    pub async fn get(&self, id: &str) -> Result<StoredTask, StoreError> {
        let row = sqlx::query("SELECT * FROM runtime_tasks WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        task_from_row(&row)
    }

    pub async fn update(&self, contract: &TaskContract) -> Result<StoredTask, StoreError> {
        let current = self.get(contract.id.as_str()).await?;
        contract.validate_for_status(current.status)?;
        let result = sqlx::query(
            "UPDATE runtime_tasks SET project_id=$2,title=$3,role=$4,objective=$5,depends_on=$6,
             allowed_paths=$7,context_refs=$8,acceptance_criteria=$9,verification_commands=$10,
             max_input_tokens=$11,max_output_tokens=$12,max_tool_calls=$13,max_attempts=$14,
             timeout_seconds=$15,version=version+1,updated_at=now() WHERE id=$1 AND version=$16",
        )
        .bind(contract.id.as_str())
        .bind(contract.project_id.as_str())
        .bind(contract.title.as_str())
        .bind(contract.role.as_str())
        .bind(contract.objective.as_str())
        .bind(serde_json::to_value(&contract.depends_on)?)
        .bind(serde_json::to_value(&contract.allowed_paths)?)
        .bind(serde_json::to_value(&contract.context_refs)?)
        .bind(serde_json::to_value(&contract.acceptance_criteria)?)
        .bind(serde_json::to_value(&contract.verification_commands)?)
        .bind(contract.limits.max_input_tokens.get())
        .bind(contract.limits.max_output_tokens.get())
        .bind(contract.limits.max_tool_calls.get())
        .bind(contract.limits.max_attempts.get())
        .bind(contract.limits.timeout_seconds.get())
        .bind(current.version)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict(Conflict::StaleTransition));
        }
        self.get(contract.id.as_str()).await
    }

    pub async fn delete(&self, id: &str) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM runtime_tasks WHERE id=$1")
            .bind(id)
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
        let rows = sqlx::query(
            "SELECT * FROM runtime_tasks WHERE ($1::text IS NULL OR id > $1)
             ORDER BY id LIMIT $2",
        )
        .bind(cursor)
        .bind(limit + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        let has_more = rows.len() > limit as usize;
        let visible = &rows[..rows.len().min(limit as usize)];
        Ok(Page {
            items: visible
                .iter()
                .map(task_from_row)
                .collect::<Result<_, _>>()?,
            next_cursor: has_more.then(|| {
                visible
                    .last()
                    .expect("page has item")
                    .get::<String, _>("id")
            }),
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
        let row = sqlx::query("SELECT * FROM runtime_tasks WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(StoreError::Database)?
            .ok_or(StoreError::NotFound)?;
        let current = task_from_row(&row)?;
        if current.version != expected_version {
            return Err(StoreError::Conflict(Conflict::StaleTransition));
        }
        transition(&current.contract, current.status, to, actor)?;
        let result = sqlx::query(
            "UPDATE runtime_tasks SET status=$2,version=version+1,updated_at=now()
             WHERE id=$1 AND version=$3",
        )
        .bind(id)
        .bind(enum_text(&to)?)
        .bind(expected_version)
        .execute(&mut *transaction)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::Conflict(Conflict::StaleTransition));
        }
        insert_event(
            &mut transaction,
            id,
            actor,
            "status_transition",
            Some(current.status),
            Some(to),
            &Value::Object(Default::default()),
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
        let rows = sqlx::query("SELECT id,task_id,actor,event_type,from_status,to_status,payload FROM task_events WHERE task_id=$1 ORDER BY id")
            .bind(task_id).fetch_all(&self.pool).await.map_err(StoreError::Database)?;
        rows.iter().map(event_from_row).collect()
    }

    pub async fn create_attempt(&self, attempt: &AgentAttempt) -> Result<(), StoreError> {
        attempt.validate()?;
        let result = sqlx::query("INSERT INTO agent_attempts (id,task_id,attempt,provider_id,model_id,status) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(attempt.id).bind(attempt.task_id.as_str()).bind(attempt.attempt)
            .bind(attempt.provider_id.as_str()).bind(attempt.model_id.as_str()).bind(attempt.status.as_str())
            .execute(&self.pool).await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if constraint(&error) == Some("agent_attempts_task_id_attempt_key") => {
                Err(StoreError::Conflict(Conflict::Attempt))
            }
            Err(error) => Err(StoreError::Database(error)),
        }
    }

    pub async fn record_usage(
        &self,
        attempt_id: uuid::Uuid,
        usage: &Usage,
    ) -> Result<(), StoreError> {
        usage.validate()?;
        sqlx::query("INSERT INTO task_usage (attempt_id,input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(attempt_id).bind(usage.input_tokens).bind(usage.cached_tokens).bind(usage.output_tokens)
            .bind(usage.tool_calls).bind(usage.latency_ms).execute(&self.pool).await.map_err(StoreError::Database)?;
        Ok(())
    }
}

async fn insert_event(
    transaction: &mut Transaction<'_, Postgres>,
    task_id: &str,
    actor: Actor,
    event_type: &str,
    from: Option<TaskStatus>,
    to: Option<TaskStatus>,
    payload: &Value,
) -> Result<(), StoreError> {
    if !payload.is_object() {
        return Err(StoreError::InvalidJson(serde_json::Error::io(
            std::io::Error::other("event payload must be object"),
        )));
    }
    sqlx::query("INSERT INTO task_events (task_id,actor,event_type,from_status,to_status,payload) VALUES ($1,$2,$3,$4,$5,$6)")
        .bind(task_id).bind(enum_text(&actor)?).bind(event_type)
        .bind(from.map(|value| enum_text(&value)).transpose()?).bind(to.map(|value| enum_text(&value)).transpose()?)
        .bind(payload).execute(&mut **transaction).await.map_err(StoreError::Database)?;
    Ok(())
}

fn task_from_row(row: &PgRow) -> Result<StoredTask, StoreError> {
    let contract = TaskContract {
        id: text(row, "id", "id")?,
        project_id: text(row, "project_id", "project_id")?,
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
    let status = parse_enum(row.try_get("status").map_err(StoreError::Database)?)?;
    contract.validate_for_status(status)?;
    Ok(StoredTask {
        contract,
        status,
        version: row.try_get("version").map_err(StoreError::Database)?,
    })
}

fn event_from_row(row: &PgRow) -> Result<TaskEvent, StoreError> {
    Ok(TaskEvent {
        id: row.try_get("id").map_err(StoreError::Database)?,
        task_id: text(row, "task_id", "task_id")?,
        actor: parse_enum(row.try_get("actor").map_err(StoreError::Database)?)?,
        event_type: text(row, "event_type", "event_type")?,
        from_status: row
            .try_get::<Option<String>, _>("from_status")
            .map_err(StoreError::Database)?
            .map(parse_enum)
            .transpose()?,
        to_status: row
            .try_get::<Option<String>, _>("to_status")
            .map_err(StoreError::Database)?
            .map(parse_enum)
            .transpose()?,
        payload: row.try_get("payload").map_err(StoreError::Database)?,
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
fn constraint(error: &sqlx::Error) -> Option<&str> {
    error
        .as_database_error()
        .and_then(|error| error.constraint())
}
