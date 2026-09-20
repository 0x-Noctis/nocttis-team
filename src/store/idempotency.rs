use serde_json::Value;
use sqlx::{PgPool, Row};

use super::provider::StoreError;

#[derive(Clone, Debug, PartialEq)]
pub enum Reservation {
    New,
    InProgress,
    Conflict,
    Replay { status_code: i16, body: Value },
}

#[derive(Clone)]
pub struct IdempotencyRepository {
    pool: PgPool,
}

impl IdempotencyRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn reserve(
        &self,
        key: &str,
        request_fingerprint: &str,
    ) -> Result<Reservation, StoreError> {
        let inserted = sqlx::query(
            "INSERT INTO idempotency_keys (key, request_fingerprint)
             VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(key)
        .bind(request_fingerprint)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?
        .rows_affected()
            == 1;
        if inserted {
            return Ok(Reservation::New);
        }

        let row = sqlx::query(
            "SELECT request_fingerprint, status_code, response_body
             FROM idempotency_keys WHERE key = $1",
        )
        .bind(key)
        .fetch_one(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if row
            .try_get::<String, _>("request_fingerprint")
            .map_err(StoreError::Database)?
            != request_fingerprint
        {
            return Ok(Reservation::Conflict);
        }
        match (
            row.try_get::<Option<i16>, _>("status_code")
                .map_err(StoreError::Database)?,
            row.try_get::<Option<Value>, _>("response_body")
                .map_err(StoreError::Database)?,
        ) {
            (Some(status_code), Some(body)) => Ok(Reservation::Replay { status_code, body }),
            _ => Ok(Reservation::InProgress),
        }
    }

    pub async fn complete(
        &self,
        key: &str,
        request_fingerprint: &str,
        status_code: i16,
        body: &Value,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE idempotency_keys
             SET status_code = $3, response_body = $4, completed_at = now()
             WHERE key = $1 AND request_fingerprint = $2 AND status_code IS NULL",
        )
        .bind(key)
        .bind(request_fingerprint)
        .bind(status_code)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub async fn cancel(&self, key: &str, request_fingerprint: &str) -> Result<(), StoreError> {
        sqlx::query(
            "DELETE FROM idempotency_keys
             WHERE key = $1 AND request_fingerprint = $2 AND status_code IS NULL",
        )
        .bind(key)
        .bind(request_fingerprint)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        Ok(())
    }
}
