use std::time::Duration;

use base_skeleton_rust::{
    application::job::{JobDisposition, JobQueue, NewJob},
    infrastructure::database::postgres::PostgresJobQueue,
};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
async fn exercises_the_postgres_job_lifecycle_when_a_test_database_is_configured() {
    let Some(pool) = test_pool().await else {
        return;
    };
    sqlx::migrate!().run(&pool).await.unwrap();

    let queue = PostgresJobQueue::new(pool.clone());
    let job = NewJob::new("test.lifecycle", json!({ "value": 42 }), 2).with_trace_context(
        json!({ "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01" }),
    );
    queue.enqueue(&job).await.unwrap();

    let first_attempt = queue
        .claim("test-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_attempt.id, job.id);
    assert_eq!(first_attempt.attempts, 1);
    assert_eq!(first_attempt.trace_context, job.trace_context);
    assert_eq!(
        queue
            .fail(job.id, "test-worker", "temporary failure", Duration::ZERO,)
            .await
            .unwrap(),
        JobDisposition::RetryScheduled
    );

    let final_attempt = queue
        .claim("test-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_attempt.id, job.id);
    assert_eq!(final_attempt.attempts, 2);
    assert_eq!(
        queue
            .fail(job.id, "test-worker", "permanent failure", Duration::ZERO,)
            .await
            .unwrap(),
        JobDisposition::DeadLettered
    );

    let (status, attempts, last_error): (String, i32, Option<String>) =
        sqlx::query_as("SELECT status, attempts, last_error FROM background_jobs WHERE id = $1")
            .bind(job.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "dead");
    assert_eq!(attempts, 2);
    assert_eq!(last_error.as_deref(), Some("permanent failure"));

    let successful_job = NewJob::new("test.complete", json!({}), 1);
    queue.enqueue(&successful_job).await.unwrap();
    let claimed = queue
        .claim("test-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, successful_job.id);
    queue
        .complete(successful_job.id, "test-worker")
        .await
        .unwrap();

    let completed_status: String =
        sqlx::query_scalar("SELECT status FROM background_jobs WHERE id = $1")
            .bind(successful_job.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(completed_status, "completed");

    sqlx::query("DELETE FROM background_jobs WHERE id = ANY($1)")
        .bind([job.id, successful_job.id])
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn purges_old_completed_and_dead_jobs_but_keeps_recent_jobs() {
    let Some(pool) = test_pool().await else {
        return;
    };
    sqlx::migrate!().run(&pool).await.unwrap();
    let old_completed = Uuid::new_v4();
    let old_dead = Uuid::new_v4();
    let recent_completed = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO background_jobs
           (id, job_type, payload, status, attempts, max_attempts, updated_at, completed_at)
           VALUES
           ($1, 'test.cleanup', '{}', 'completed', 1, 1, NOW() - INTERVAL '2 days', NOW() - INTERVAL '2 days'),
           ($2, 'test.cleanup', '{}', 'dead', 1, 1, NOW() - INTERVAL '31 days', NULL),
           ($3, 'test.cleanup', '{}', 'completed', 1, 1, NOW(), NOW())"#,
    )
    .bind(old_completed)
    .bind(old_dead)
    .bind(recent_completed)
    .execute(&pool)
    .await
    .unwrap();

    let queue = PostgresJobQueue::new(pool.clone());
    assert_eq!(
        queue
            .purge_terminal(Duration::from_secs(86_400), Duration::from_secs(2_592_000))
            .await
            .unwrap(),
        2
    );
    let remaining: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM background_jobs WHERE id = ANY($1) ORDER BY id")
            .bind([old_completed, old_dead, recent_completed])
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, vec![recent_completed]);

    sqlx::query("DELETE FROM background_jobs WHERE id = $1")
        .bind(recent_completed)
        .execute(&pool)
        .await
        .unwrap();
}

async fn test_pool() -> Option<PgPool> {
    let database_url = match std::env::var("TEST_DATABASE_URL") {
        Ok(value) => value,
        Err(_) if std::env::var("CI").is_ok_and(|value| value == "true") => {
            panic!("TEST_DATABASE_URL must be configured in CI")
        }
        Err(_) => {
            eprintln!("skipping PostgreSQL integration test: TEST_DATABASE_URL is not configured");
            return None;
        }
    };
    Some(
        PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("could not connect to TEST_DATABASE_URL"),
    )
}
