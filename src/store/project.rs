use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::{
    project::{
        ApprovalDecision, PlanApproval, PlanStatus, Project, ProjectRun, ProposedPlan, RunStatus,
    },
    task::{NonEmptyString, PositiveLimit, ValidationError},
};

#[derive(Debug)]
pub enum ProjectStoreError {
    NotFound,
    Conflict,
    Invalid(ValidationError),
    InvalidId(&'static str),
    Database(sqlx::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for ProjectStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("resource not found"),
            Self::Conflict => formatter.write_str("project plan conflict"),
            Self::Invalid(error) => error.fmt(formatter),
            Self::InvalidId(field) => write!(formatter, "{field} must be a canonical UUID"),
            Self::Database(_) => formatter.write_str("database operation failed"),
            Self::Json(_) => formatter.write_str("invalid plan JSON"),
        }
    }
}

impl std::error::Error for ProjectStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(error) => Some(error),
            Self::Database(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<sqlx::Error> for ProjectStoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}
impl From<serde_json::Error> for ProjectStoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
impl From<ValidationError> for ProjectStoreError {
    fn from(error: ValidationError) -> Self {
        Self::Invalid(error)
    }
}

fn uuid(field: &'static str, value: &str) -> Result<Uuid, ProjectStoreError> {
    let id = Uuid::parse_str(value).map_err(|_| ProjectStoreError::InvalidId(field))?;
    if id.to_string() != value {
        return Err(ProjectStoreError::InvalidId(field));
    }
    Ok(id)
}

fn status(value: RunStatus) -> &'static str {
    match value {
        RunStatus::Planning => "PLANNING",
        RunStatus::AwaitingApproval => "AWAITING_APPROVAL",
        RunStatus::Running => "RUNNING",
        RunStatus::Paused => "PAUSED",
        RunStatus::Done => "DONE",
        RunStatus::Cancelled => "CANCELLED",
        RunStatus::Failed => "FAILED",
    }
}

fn conflict(error: sqlx::Error) -> ProjectStoreError {
    if error
        .as_database_error()
        .is_some_and(|db| db.is_unique_violation())
    {
        ProjectStoreError::Conflict
    } else {
        ProjectStoreError::Database(error)
    }
}

#[derive(Clone)]
pub struct ProjectRepository {
    pool: PgPool,
}

impl ProjectRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create_project(&self, project: &Project) -> Result<(), ProjectStoreError> {
        let id = uuid("id", project.id.as_str())?;
        sqlx::query("INSERT INTO projects (id,name,repository_path) VALUES ($1,$2,$3)")
            .bind(id)
            .bind(project.name.as_str())
            .bind(project.repository_path.as_path().to_str())
            .execute(&self.pool)
            .await
            .map_err(conflict)?;
        Ok(())
    }

    pub async fn get_project(&self, id: &str) -> Result<Project, ProjectStoreError> {
        let row = sqlx::query("SELECT name,repository_path FROM projects WHERE id=$1")
            .bind(uuid("id", id)?)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(ProjectStoreError::NotFound)?;
        Ok(Project {
            id: NonEmptyString::parse("id", id)?,
            name: NonEmptyString::parse("name", row.get::<String, _>("name"))?,
            repository_path: row.get::<String, _>("repository_path").parse()?,
        })
    }

    pub async fn create_run(&self, run: &ProjectRun) -> Result<(), ProjectStoreError> {
        run.validate()?;
        let id = uuid("id", run.id.as_str())?;
        let project_id = uuid("project_id", run.project_id.as_str())?;
        if run.status != RunStatus::Planning {
            return Err(ProjectStoreError::Conflict);
        }
        sqlx::query("INSERT INTO project_runs (id,project_id,objective,status,token_budget,acceptance_criteria) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(id).bind(project_id).bind(run.objective.as_str()).bind(status(run.status))
            .bind(run.token_budget.get()).bind(serde_json::to_value(&run.acceptance_criteria)?)
            .execute(&self.pool).await.map_err(conflict)?;
        Ok(())
    }

    pub async fn get_run(&self, id: &str) -> Result<ProjectRun, ProjectStoreError> {
        let row = sqlx::query("SELECT project_id,objective,status,token_budget,acceptance_criteria FROM project_runs WHERE id=$1")
            .bind(uuid("id", id)?).fetch_optional(&self.pool).await?
            .ok_or(ProjectStoreError::NotFound)?;
        let state = match row.get::<&str, _>("status") {
            "PLANNING" => RunStatus::Planning,
            "AWAITING_APPROVAL" => RunStatus::AwaitingApproval,
            "RUNNING" => RunStatus::Running,
            "PAUSED" => RunStatus::Paused,
            "DONE" => RunStatus::Done,
            "CANCELLED" => RunStatus::Cancelled,
            "FAILED" => RunStatus::Failed,
            _ => return Err(ProjectStoreError::Conflict),
        };
        Ok(ProjectRun {
            id: NonEmptyString::parse("id", id)?,
            project_id: NonEmptyString::parse(
                "project_id",
                row.get::<Uuid, _>("project_id").to_string(),
            )?,
            objective: NonEmptyString::parse("objective", row.get::<String, _>("objective"))?,
            acceptance_criteria: serde_json::from_value(row.get("acceptance_criteria"))?,
            token_budget: PositiveLimit::new("token_budget", row.get("token_budget"))?,
            status: state,
        })
    }

    pub async fn propose(&self, plan: &ProposedPlan) -> Result<(), ProjectStoreError> {
        plan.validate()?;
        if plan.status != PlanStatus::Proposed {
            return Err(ProjectStoreError::Conflict);
        }
        let run_id = uuid("project_run_id", plan.project_run_id.as_str())?;
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT status FROM project_runs WHERE id=$1 FOR UPDATE")
            .bind(run_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ProjectStoreError::NotFound)?;
        if !matches!(
            row.get::<&str, _>("status"),
            "PLANNING" | "AWAITING_APPROVAL" | "RUNNING" | "PAUSED"
        ) {
            return Err(ProjectStoreError::Conflict);
        }
        let next: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version),0)+1 FROM plans WHERE project_run_id=$1",
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
        if next != plan.version.get() {
            return Err(ProjectStoreError::Conflict);
        }
        if sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM plans WHERE project_run_id=$1 AND status='PROPOSED'",
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?
            != 0
        {
            return Err(ProjectStoreError::Conflict);
        }
        let project_id: Uuid =
            sqlx::query_scalar("SELECT project_id FROM project_runs WHERE id=$1")
                .bind(run_id)
                .fetch_one(&mut *tx)
                .await?;
        for task in &plan.tasks {
            if uuid("project_id", task.project_id.as_str())? != project_id {
                return Err(ProjectStoreError::Conflict);
            }
            if sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=$1)")
                .bind(task.id.as_str())
                .fetch_one(&mut *tx)
                .await?
            {
                return Err(ProjectStoreError::Conflict);
            }
        }
        sqlx::query("INSERT INTO plans (id,project_run_id,version,tasks,risk_flags) VALUES ($1,$2,$3,$4,$5)")
            .bind(plan.id.as_str()).bind(run_id).bind(plan.version.get())
            .bind(serde_json::to_value(&plan.tasks)?).bind(serde_json::to_value(&plan.risk_flags)?)
            .execute(&mut *tx).await.map_err(conflict)?;
        sqlx::query("UPDATE project_runs SET status='AWAITING_APPROVAL',updated_at=now() WHERE id=$1 AND status='PLANNING'")
            .bind(run_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_plan(&self, id: &str) -> Result<ProposedPlan, ProjectStoreError> {
        let row = sqlx::query(
            "SELECT project_run_id,version,tasks,risk_flags,status FROM plans WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ProjectStoreError::NotFound)?;
        let state = match row.get::<&str, _>("status") {
            "PROPOSED" => PlanStatus::Proposed,
            "APPROVED" | "SUPERSEDED" => PlanStatus::Approved,
            "REJECTED" => PlanStatus::Rejected,
            _ => return Err(ProjectStoreError::Conflict),
        };
        Ok(ProposedPlan {
            id: NonEmptyString::parse("id", id)?,
            project_run_id: NonEmptyString::parse(
                "project_run_id",
                row.get::<Uuid, _>("project_run_id").to_string(),
            )?,
            version: PositiveLimit::new("version", row.get("version"))?,
            tasks: serde_json::from_value(row.get("tasks"))?,
            risk_flags: serde_json::from_value(row.get("risk_flags"))?,
            status: state,
        })
    }

    pub async fn decide(&self, approval: &PlanApproval) -> Result<(), ProjectStoreError> {
        approval.validate()?;
        let mut tx = self.pool.begin().await?;
        // Kunci run lebih dulu agar proposal dan approval bersaing secara serial.
        let run_id: Uuid = sqlx::query_scalar("SELECT project_run_id FROM plans WHERE id=$1")
            .bind(approval.plan_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(ProjectStoreError::NotFound)?;
        let row = sqlx::query(
            "SELECT token_budget,reserved_tokens,status FROM project_runs WHERE id=$1 FOR UPDATE",
        )
        .bind(run_id)
        .fetch_one(&mut *tx)
        .await?;
        let plan = sqlx::query("SELECT tasks,status FROM plans WHERE id=$1 FOR UPDATE")
            .bind(approval.plan_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
        if plan.get::<&str, _>("status") != "PROPOSED"
            || !matches!(
                row.get::<&str, _>("status"),
                "AWAITING_APPROVAL" | "RUNNING" | "PAUSED"
            )
        {
            return Err(ProjectStoreError::Conflict);
        }
        if approval.decision == ApprovalDecision::Rejected {
            sqlx::query("UPDATE plans SET status='REJECTED',actor_id=$2,reason=$3 WHERE id=$1")
                .bind(approval.plan_id.as_str())
                .bind(approval.actor_id.as_str())
                .bind(approval.reason.as_ref().map(NonEmptyString::as_str))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(());
        }
        let tasks: Vec<crate::domain::task::TaskContract> =
            serde_json::from_value(plan.get("tasks"))?;
        // ponytail: reservasi memakai batas input+output per task; rekonsiliasi usage masuk M4 budget guard.
        let requested = tasks.iter().try_fold(0_i64, |sum, task| {
            sum.checked_add(task.limits.max_input_tokens.get())
                .and_then(|value| value.checked_add(task.limits.max_output_tokens.get()))
                .ok_or(ProjectStoreError::Conflict)
        })?;
        let reserved: i64 = row.get("reserved_tokens");
        let budget: i64 = row.get("token_budget");
        let available = budget
            .checked_sub(reserved)
            .ok_or(ProjectStoreError::Conflict)?;
        if requested > available {
            return Err(ProjectStoreError::Conflict);
        }
        for task in &tasks {
            sqlx::query("INSERT INTO tasks (id,project_run_id,role,title,objective,status,allowed_paths,acceptance_criteria,verification_commands,max_input_tokens,max_output_tokens,max_attempts,context_refs,max_tool_calls,timeout_seconds,plan_id) VALUES ($1,$2,$3,$4,$5,'READY',$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
                .bind(task.id.as_str()).bind(run_id).bind(task.role.as_str()).bind(task.title.as_str())
                .bind(task.objective.as_str()).bind(serde_json::to_value(&task.allowed_paths)?)
                .bind(serde_json::to_value(&task.acceptance_criteria)?)
                .bind(serde_json::to_value(&task.verification_commands)?)
                .bind(task.limits.max_input_tokens.get()).bind(task.limits.max_output_tokens.get())
                .bind(i16::try_from(task.limits.max_attempts.get()).map_err(|_| ProjectStoreError::Conflict)?)
                .bind(serde_json::to_value(&task.context_refs)?)
                .bind(task.limits.max_tool_calls.get()).bind(task.limits.timeout_seconds.get())
                .bind(approval.plan_id.as_str()).execute(&mut *tx).await.map_err(conflict)?;
        }
        for task in &tasks {
            for dependency in &task.depends_on {
                sqlx::query("INSERT INTO task_dependencies (task_id,dependency_id) VALUES ($1,$2)")
                    .bind(task.id.as_str())
                    .bind(dependency.as_str())
                    .execute(&mut *tx)
                    .await?;
            }
        }
        sqlx::query(
            "UPDATE plans SET status='SUPERSEDED' WHERE project_run_id=$1 AND status='APPROVED'",
        )
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE plans SET status='APPROVED',actor_id=$2,reason=$3 WHERE id=$1")
            .bind(approval.plan_id.as_str())
            .bind(approval.actor_id.as_str())
            .bind(approval.reason.as_ref().map(NonEmptyString::as_str))
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE project_runs SET reserved_tokens=$2,status='RUNNING',updated_at=now() WHERE id=$1")
            .bind(run_id).bind(reserved + requested).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
}
