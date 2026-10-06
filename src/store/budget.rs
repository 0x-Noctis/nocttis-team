//! Reservasi dan rekonsiliasi budget token (M4-003).
//!
//! Alur satu request model: `reserve` (tahan token, ditolak bila melewati batas run/task/attempt) ->
//! kirim request -> `settle` (catat usage nyata, lepaskan selisihnya) atau `settle_estimated` (provider
//! tidak memberi usage) atau `release` (request gagal, kembalikan seluruhnya).
//!
//! Semua keputusan satu run diserialkan oleh `SELECT ... FOR UPDATE` pada baris `project_runs`, sehingga
//! reservasi paralel tidak pernah melewati batas (tidak ada oversubscribe). Transaksi singkat dan tidak
//! memanggil model di dalamnya.

use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    domain::budget::{
        BudgetLevel, Denial, Levels, Position, Purpose, Scope, decide, level, project_reserve,
    },
    store::event::Usage,
};

#[derive(Debug)]
pub enum BudgetError {
    /// Request tidak boleh dikirim; tidak ada yang ditahan.
    Denied(Denial),
    NotFound,
    /// Attempt sudah selesai atau reservasi sudah ditutup dengan cara lain.
    Closed,
    InvalidInput(&'static str),
    Database(sqlx::Error),
}

impl std::fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied(_) => formatter.write_str("token budget does not allow this request"),
            Self::NotFound => formatter.write_str("budget subject not found"),
            Self::Closed => formatter.write_str("attempt or reservation is already closed"),
            Self::InvalidInput(field) => write!(formatter, "invalid budget input: {field}"),
            Self::Database(_) => formatter.write_str("database operation failed"),
        }
    }
}

impl std::error::Error for BudgetError {}

impl From<sqlx::Error> for BudgetError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

pub struct ReserveRequest<'a> {
    pub attempt_id: Uuid,
    /// Kunci idempotensi request dalam attempt; mengulang kunci yang masih ditahan mengembalikan
    /// reservasi yang sama.
    pub request_key: &'a str,
    pub input_tokens: i64,
    pub max_output_tokens: i64,
    pub purpose: Purpose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub id: Uuid,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub levels: Levels,
}

/// Ringkasan budget satu run untuk dashboard dan keputusan scheduler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunBudgetView {
    pub limit: i64,
    /// Bagian yang hanya boleh dipakai recovery/integrasi.
    pub reserve: i64,
    pub used: i64,
    pub held: i64,
    pub estimated: bool,
    /// Level terhadap batas untuk pekerjaan biasa (limit - reserve).
    pub level: BudgetLevel,
}

#[derive(Clone)]
pub struct BudgetStore {
    pool: PgPool,
}

struct Subject {
    run_id: Uuid,
    task_id: String,
    run_limit: i64,
    task_limit: i64,
    input_limit: i64,
    output_limit: i64,
}

impl BudgetStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn reserve(&self, request: &ReserveRequest<'_>) -> Result<Reservation, BudgetError> {
        if request.request_key.trim().is_empty() {
            return Err(BudgetError::InvalidInput("request_key"));
        }
        if request.input_tokens < 0 || request.max_output_tokens < 0 {
            return Err(BudgetError::InvalidInput("tokens"));
        }
        let mut tx = self.pool.begin().await?;
        let subject = lock_subject(&mut tx, request.attempt_id).await?;

        // Pengulangan kunci yang sama: reservasi yang masih ditahan dikembalikan apa adanya (idempotent).
        if let Some(row) = sqlx::query(
            "SELECT id,input_tokens,output_tokens,status FROM budget_reservations WHERE agent_run_id=$1 AND request_key=$2",
        )
        .bind(request.attempt_id)
        .bind(request.request_key)
        .fetch_optional(&mut *tx)
        .await?
        {
            if row.get::<&str, _>("status") != "held" {
                return Err(BudgetError::Closed);
            }
            let position = position(&mut tx, &subject, request.attempt_id).await?;
            // Reservasi sendiri sudah masuk `committed`; level dihitung tanpa menambahnya lagi.
            let levels = decide(0, 0, request.purpose, &position).unwrap_or(Levels {
                run: BudgetLevel::Stop,
                task: BudgetLevel::Stop,
                attempt: BudgetLevel::Stop,
            });
            return Ok(Reservation {
                id: row.get("id"),
                input_tokens: row.get("input_tokens"),
                output_tokens: row.get("output_tokens"),
                levels,
            });
        }

        let position = position(&mut tx, &subject, request.attempt_id).await?;
        let levels = decide(
            request.input_tokens,
            request.max_output_tokens,
            request.purpose,
            &position,
        )
        .map_err(BudgetError::Denied)?;
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO budget_reservations (id,project_run_id,task_id,agent_run_id,request_key,input_tokens,output_tokens,purpose)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(id)
        .bind(subject.run_id)
        .bind(&subject.task_id)
        .bind(request.attempt_id)
        .bind(request.request_key)
        .bind(request.input_tokens)
        .bind(request.max_output_tokens)
        .bind(match request.purpose {
            Purpose::Work => "work",
            Purpose::Recovery => "recovery",
        })
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Reservation {
            id,
            input_tokens: request.input_tokens,
            output_tokens: request.max_output_tokens,
            levels,
        })
    }

    /// Tutup reservasi dengan usage nyata dari provider. Pemakaian yang melebihi reservasi tetap dicatat
    /// apa adanya (request sudah terjadi); level berikutnya akan menolak request baru. Idempotent: reservasi
    /// yang sudah settled mengembalikan level terkini tanpa mencatat usage lagi.
    pub async fn settle(&self, reservation_id: Uuid, usage: &Usage) -> Result<Levels, BudgetError> {
        self.close(reservation_id, Some(usage.clone())).await
    }

    /// Provider tidak memberi usage: bebankan seluruh reservasi (input + batas output) sebagai estimasi.
    /// Sengaja konservatif supaya budget tidak pernah terhitung lebih kecil dari pemakaian sebenarnya.
    pub async fn settle_estimated(&self, reservation_id: Uuid) -> Result<Levels, BudgetError> {
        self.close(reservation_id, None).await
    }

    async fn close(
        &self,
        reservation_id: Uuid,
        usage: Option<Usage>,
    ) -> Result<Levels, BudgetError> {
        let mut tx = self.pool.begin().await?;
        let attempt_id = reservation_attempt(&mut tx, reservation_id).await?;
        let subject = lock_subject_any(&mut tx, attempt_id).await?;
        let row = sqlx::query(
            "SELECT request_key,input_tokens,output_tokens,status,purpose FROM budget_reservations WHERE id=$1 FOR UPDATE",
        )
        .bind(reservation_id)
        .fetch_one(&mut *tx)
        .await?;
        match row.get::<&str, _>("status") {
            "settled" => {}
            "released" => return Err(BudgetError::Closed),
            _ => {
                let usage = usage.unwrap_or(Usage {
                    input_tokens: row.get("input_tokens"),
                    cached_tokens: 0,
                    output_tokens: row.get("output_tokens"),
                    tool_calls: 0,
                    latency_ms: 0,
                    estimated: true,
                });
                if usage.input_tokens < 0 || usage.output_tokens < 0 || usage.cached_tokens < 0 {
                    return Err(BudgetError::InvalidInput("usage"));
                }
                // Tutup reservasi dan catat usage dalam satu transaksi: tidak ada saat di mana token
                // terhitung dua kali (ditahan + terpakai) atau hilang dari hitungan.
                sqlx::query(
                    "UPDATE budget_reservations SET status='settled',closed_at=now() WHERE id=$1",
                )
                .bind(reservation_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "INSERT INTO model_usage (agent_run_id,operation_key,input_tokens,cached_tokens,output_tokens,tool_calls,latency_ms,estimated)
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (agent_run_id,operation_key) WHERE operation_key IS NOT NULL DO NOTHING",
                )
                .bind(attempt_id)
                .bind(row.get::<&str, _>("request_key"))
                .bind(usage.input_tokens)
                .bind(usage.cached_tokens)
                .bind(usage.output_tokens)
                .bind(usage.tool_calls)
                .bind(usage.latency_ms)
                .bind(usage.estimated)
                .execute(&mut *tx)
                .await?;
            }
        }
        let position = position(&mut tx, &subject, attempt_id).await?;
        tx.commit().await?;
        Ok(levels_of(&position))
    }

    /// Request gagal sebelum menghasilkan usage: kembalikan seluruh token yang ditahan. Idempotent.
    pub async fn release(&self, reservation_id: Uuid) -> Result<(), BudgetError> {
        let released = sqlx::query(
            "UPDATE budget_reservations SET status='released',closed_at=now() WHERE id=$1 AND status='held'",
        )
        .bind(reservation_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if released == 1 {
            return Ok(());
        }
        match sqlx::query_scalar::<_, String>("SELECT status FROM budget_reservations WHERE id=$1")
            .bind(reservation_id)
            .fetch_optional(&self.pool)
            .await?
            .as_deref()
        {
            Some("released") => Ok(()),
            Some(_) => Err(BudgetError::Closed),
            None => Err(BudgetError::NotFound),
        }
    }

    /// Lepas semua reservasi yang masih ditahan sebuah attempt (attempt mati/dipulihkan). Mengembalikan jumlahnya.
    pub async fn release_attempt(&self, attempt_id: Uuid) -> Result<u64, BudgetError> {
        Ok(sqlx::query(
            "UPDATE budget_reservations SET status='released',closed_at=now() WHERE agent_run_id=$1 AND status='held'",
        )
        .bind(attempt_id)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    pub async fn run_view(&self, run_id: Uuid) -> Result<RunBudgetView, BudgetError> {
        let limit: i64 = sqlx::query_scalar("SELECT token_budget FROM project_runs WHERE id=$1")
            .bind(run_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(BudgetError::NotFound)?;
        let (used, estimated) = run_used(&self.pool, run_id).await?;
        let held: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(input_tokens+output_tokens),0)::bigint FROM budget_reservations WHERE project_run_id=$1 AND status='held'",
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?;
        let reserve = project_reserve(limit);
        Ok(RunBudgetView {
            limit,
            reserve,
            used,
            held,
            estimated,
            level: level(used.saturating_add(held), limit.saturating_sub(reserve)),
        })
    }
}

fn levels_of(position: &Position) -> Levels {
    // Level pasca-settle dihitung terhadap batas pekerjaan biasa; penambahan 0 tidak pernah ditolak
    // kecuali scope sudah penuh, jadi level penuh dilaporkan sebagai Stop.
    let work = |scope: Scope| level(scope.committed, scope.limit);
    Levels {
        run: level(
            position.run.committed,
            position
                .run
                .limit
                .saturating_sub(project_reserve(position.run.limit)),
        ),
        task: work(position.task),
        attempt: level(
            position
                .attempt_input
                .committed
                .saturating_add(position.attempt_output.committed),
            position
                .attempt_input
                .limit
                .saturating_add(position.attempt_output.limit),
        ),
    }
}

async fn reservation_attempt(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Uuid, BudgetError> {
    sqlx::query_scalar("SELECT agent_run_id FROM budget_reservations WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(BudgetError::NotFound)
}

/// Kunci baris run milik attempt aktif. Attempt yang sudah selesai tidak boleh menahan token baru.
async fn lock_subject(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: Uuid,
) -> Result<Subject, BudgetError> {
    let subject = lock_subject_any(tx, attempt_id).await?;
    let active: bool = sqlx::query_scalar("SELECT finished_at IS NULL FROM agent_runs WHERE id=$1")
        .bind(attempt_id)
        .fetch_one(&mut **tx)
        .await?;
    if active {
        Ok(subject)
    } else {
        Err(BudgetError::Closed)
    }
}

async fn lock_subject_any(
    tx: &mut Transaction<'_, Postgres>,
    attempt_id: Uuid,
) -> Result<Subject, BudgetError> {
    let run_id: Uuid = sqlx::query_scalar(
        "SELECT t.project_run_id FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE a.id=$1",
    )
    .bind(attempt_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(BudgetError::NotFound)?;
    let run_limit: i64 =
        sqlx::query_scalar("SELECT token_budget FROM project_runs WHERE id=$1 FOR UPDATE")
            .bind(run_id)
            .fetch_one(&mut **tx)
            .await?;
    let task = sqlx::query(
        "SELECT t.id,t.max_input_tokens,t.max_output_tokens,t.max_attempts FROM agent_runs a JOIN tasks t ON t.id=a.task_id WHERE a.id=$1",
    )
    .bind(attempt_id)
    .fetch_one(&mut **tx)
    .await?;
    let (input, output): (i64, i64) = (task.get("max_input_tokens"), task.get("max_output_tokens"));
    let attempts = i64::from(task.get::<i16, _>("max_attempts"));
    Ok(Subject {
        run_id,
        task_id: task.get("id"),
        run_limit,
        task_limit: input.saturating_add(output).saturating_mul(attempts),
        input_limit: input,
        output_limit: output,
    })
}

async fn run_used<'e>(
    executor: impl sqlx::PgExecutor<'e>,
    run_id: Uuid,
) -> Result<(i64, bool), BudgetError> {
    let row = sqlx::query(
        "SELECT COALESCE(SUM(u.input_tokens+u.output_tokens),0)::bigint AS used,
                COALESCE(bool_or(u.estimated),false) AS estimated
         FROM model_usage u JOIN agent_runs a ON a.id=u.agent_run_id JOIN tasks t ON t.id=a.task_id
         WHERE t.project_run_id=$1",
    )
    .bind(run_id)
    .fetch_one(executor)
    .await?;
    Ok((row.get("used"), row.get("estimated")))
}

/// Posisi pemakaian (usage tercatat + reservasi ditahan) di keempat scope. Dipanggil saat baris run terkunci.
async fn position(
    tx: &mut Transaction<'_, Postgres>,
    subject: &Subject,
    attempt_id: Uuid,
) -> Result<Position, BudgetError> {
    let (run_used, _) = run_used(&mut **tx, subject.run_id).await?;
    let task_used: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(u.input_tokens+u.output_tokens),0)::bigint FROM model_usage u
         JOIN agent_runs a ON a.id=u.agent_run_id WHERE a.task_id=$1",
    )
    .bind(&subject.task_id)
    .fetch_one(&mut **tx)
    .await?;
    let attempt = sqlx::query(
        "SELECT COALESCE(SUM(input_tokens),0)::bigint AS input,COALESCE(SUM(output_tokens),0)::bigint AS output
         FROM model_usage WHERE agent_run_id=$1",
    )
    .bind(attempt_id)
    .fetch_one(&mut **tx)
    .await?;
    let held = sqlx::query(
        "SELECT COALESCE(SUM(input_tokens+output_tokens),0)::bigint AS run_held,
                COALESCE(SUM(input_tokens+output_tokens) FILTER (WHERE task_id=$2),0)::bigint AS task_held,
                COALESCE(SUM(input_tokens) FILTER (WHERE agent_run_id=$3),0)::bigint AS attempt_input,
                COALESCE(SUM(output_tokens) FILTER (WHERE agent_run_id=$3),0)::bigint AS attempt_output
         FROM budget_reservations WHERE project_run_id=$1 AND status='held'",
    )
    .bind(subject.run_id)
    .bind(&subject.task_id)
    .bind(attempt_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Position {
        run: Scope {
            limit: subject.run_limit,
            committed: run_used.saturating_add(held.get("run_held")),
        },
        task: Scope {
            limit: subject.task_limit,
            committed: task_used.saturating_add(held.get("task_held")),
        },
        attempt_input: Scope {
            limit: subject.input_limit,
            committed: attempt
                .get::<i64, _>("input")
                .saturating_add(held.get("attempt_input")),
        },
        attempt_output: Scope {
            limit: subject.output_limit,
            committed: attempt
                .get::<i64, _>("output")
                .saturating_add(held.get("attempt_output")),
        },
    })
}
