use ai_team::store::{
    idempotency::{IdempotencyRepository, Reservation},
    provider::StoreError,
};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn reservation_replays_only_matching_completed_request(pool: PgPool) {
    let repository = IdempotencyRepository::new(pool);
    assert_eq!(
        repository
            .reserve("key-1", "POST:/providers:body-a")
            .await
            .unwrap(),
        Reservation::New
    );
    assert_eq!(
        repository
            .reserve("key-1", "POST:/providers:body-a")
            .await
            .unwrap(),
        Reservation::InProgress
    );
    assert_eq!(
        repository
            .reserve("key-1", "POST:/providers:body-b")
            .await
            .unwrap(),
        Reservation::Conflict
    );

    let body = json!({"id": "provider-a"});
    repository
        .complete("key-1", "POST:/providers:body-a", 201, &body)
        .await
        .unwrap();
    assert_eq!(
        repository
            .reserve("key-1", "POST:/providers:body-a")
            .await
            .unwrap(),
        Reservation::Replay {
            status_code: 201,
            body
        }
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn cancelled_reservation_can_be_retried(pool: PgPool) {
    let repository = IdempotencyRepository::new(pool);
    assert_eq!(
        repository.reserve("key-2", "request").await.unwrap(),
        Reservation::New
    );
    repository.cancel("key-2", "request").await.unwrap();
    assert_eq!(
        repository.reserve("key-2", "request").await.unwrap(),
        Reservation::New
    );
    assert!(matches!(
        repository
            .complete("missing", "request", 200, &json!({}))
            .await,
        Err(StoreError::NotFound)
    ));
}
