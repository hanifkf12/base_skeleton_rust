use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use base_skeleton_rust::{
    application::job::{
        ClaimedJob, JobDisposition, JobHandler, JobHandlerError, JobQueue, JobQueueError,
        JobTracer, JobWorker, JobWorkerConfig, NewJob, RunOutcome,
    },
    infrastructure::database::postgres::PostgresJobQueue,
};
use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tracing::Span;
use uuid::Uuid;

#[tokio::test]
async fn exercises_the_postgres_job_lifecycle_when_a_test_database_is_configured() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let pool = database.pool.clone();

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
            .fail(
                job.id,
                "test-worker",
                first_attempt.attempts,
                "temporary failure",
                Duration::ZERO
            )
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
            .fail(
                job.id,
                "test-worker",
                final_attempt.attempts,
                "permanent failure",
                Duration::ZERO
            )
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
        .complete(successful_job.id, "test-worker", claimed.attempts)
        .await
        .unwrap();

    let completed_status: String =
        sqlx::query_scalar("SELECT status FROM background_jobs WHERE id = $1")
            .bind(successful_job.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(completed_status, "completed");

    database.cleanup().await;
}

#[tokio::test]
async fn purges_old_completed_and_dead_jobs_but_keeps_recent_jobs() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let pool = database.pool.clone();
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

    database.cleanup().await;
}

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    schema: String,
}

impl TestDatabase {
    async fn new() -> Option<Self> {
        let database_url = match std::env::var("TEST_DATABASE_URL") {
            Ok(value) => value,
            Err(_) if std::env::var("CI").is_ok_and(|value| value != "false" && value != "0") => {
                panic!("TEST_DATABASE_URL must be configured in CI")
            }
            Err(_) => {
                eprintln!(
                    "skipping PostgreSQL integration test: TEST_DATABASE_URL is not configured"
                );
                return None;
            }
        };
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("could not connect to TEST_DATABASE_URL");
        let schema = format!("queue_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let search_path = format!("SET search_path TO {schema}");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |connection, _| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::query(&search_path).execute(connection).await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        Some(Self {
            pool,
            admin,
            schema,
        })
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}

struct NoOpJobTracer;

impl JobTracer for NoOpJobTracer {
    fn span(&self, _job: &ClaimedJob) -> Span {
        Span::none()
    }
}

fn worker(queue: Arc<dyn JobQueue>) -> JobWorker {
    execution_worker(queue, Vec::new(), Duration::from_secs(30))
}

fn execution_worker(
    queue: Arc<dyn JobQueue>,
    handlers: Vec<Arc<dyn JobHandler>>,
    lease_timeout: Duration,
) -> JobWorker {
    JobWorker::new(
        queue,
        handlers,
        Arc::new(NoOpJobTracer),
        JobWorkerConfig {
            worker_id: "test-worker".to_owned(),
            lease_timeout,
            retry_base: Duration::from_secs(5),
            retry_max: Duration::from_secs(60),
            completed_retention: Duration::from_secs(86_400),
            dead_retention: Duration::from_secs(2_592_000),
        },
    )
}

#[tokio::test]
async fn concurrent_claims_uniquely_own_jobs() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let queue = Arc::new(PostgresJobQueue::new(database.pool.clone()));
    let mut ids = Vec::new();
    for _ in 0..16 {
        let job = NewJob::new("test.concurrent", json!({}), 3);
        queue.enqueue(&job).await.unwrap();
        ids.push(job.id);
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(16));
    let mut claims = Vec::new();
    for index in 0..16 {
        let queue = queue.clone();
        let barrier = barrier.clone();
        claims.push(tokio::spawn(async move {
            barrier.wait().await;
            let owner = format!("worker-{index}");
            let job = queue
                .claim(&owner, Duration::from_secs(30))
                .await
                .unwrap()
                .unwrap();
            (job.id, owner, job.attempts)
        }));
    }
    let mut claimed_ids = Vec::new();
    for claim in claims {
        let (id, owner, attempt) = claim.await.unwrap();
        let state: (String, String, i32) =
            sqlx::query_as("SELECT status, locked_by, attempts FROM background_jobs WHERE id = $1")
                .bind(id)
                .fetch_one(&database.pool)
                .await
                .unwrap();
        assert_eq!(state, ("running".to_owned(), owner, 1));
        assert_eq!(attempt, 1);
        claimed_ids.push(id);
    }
    ids.sort_unstable();
    claimed_ids.sort_unstable();
    assert_eq!(claimed_ids, ids);
    assert!(
        queue
            .claim("extra", Duration::from_secs(30))
            .await
            .unwrap()
            .is_none()
    );
    database.cleanup().await;
}

#[tokio::test]
async fn reclaimed_attempt_fences_stale_writes_even_with_the_same_worker_id() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let queue = PostgresJobQueue::new(database.pool.clone());
    let job = NewJob::new("test.fence", json!({}), 3);
    queue.enqueue(&job).await.unwrap();
    let first = queue
        .claim("reused-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE background_jobs SET locked_at = NOW() - INTERVAL '31 seconds' WHERE id = $1",
    )
    .bind(job.id)
    .execute(&database.pool)
    .await
    .unwrap();
    let second = queue
        .claim("reused-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.attempts, 2);
    let before: (String, i32, String, String, Option<String>) = sqlx::query_as(
        "SELECT status, attempts, locked_by, locked_at::text, last_error FROM background_jobs WHERE id = $1",
    ).bind(job.id).fetch_one(&database.pool).await.unwrap();
    assert_eq!(
        queue
            .complete(job.id, "reused-worker", first.attempts)
            .await,
        Err(JobQueueError::LeaseLost)
    );
    assert_eq!(
        queue
            .fail(
                job.id,
                "reused-worker",
                first.attempts,
                "stale failure",
                Duration::ZERO
            )
            .await,
        Err(JobQueueError::LeaseLost)
    );
    assert_eq!(
        queue.renew(job.id, "reused-worker", first.attempts).await,
        Err(JobQueueError::LeaseLost)
    );
    let after: (String, i32, String, String, Option<String>) = sqlx::query_as(
        "SELECT status, attempts, locked_by, locked_at::text, last_error FROM background_jobs WHERE id = $1",
    ).bind(job.id).fetch_one(&database.pool).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(after.0, "running");

    sqlx::query(
        "UPDATE background_jobs SET locked_at = NOW() - INTERVAL '31 seconds' WHERE id = $1",
    )
    .bind(job.id)
    .execute(&database.pool)
    .await
    .unwrap();
    queue
        .renew(job.id, "reused-worker", second.attempts)
        .await
        .unwrap();
    assert!(
        queue
            .claim("competitor", Duration::from_secs(30))
            .await
            .unwrap()
            .is_none()
    );
    queue
        .complete(job.id, "reused-worker", second.attempts)
        .await
        .unwrap();
    let state: (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM background_jobs WHERE id = $1")
            .bind(job.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(state, ("completed".to_owned(), 2));
    database.cleanup().await;
}

#[tokio::test]
async fn expired_final_attempt_is_dead_lettered_during_claim() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let queue = PostgresJobQueue::new(database.pool.clone());
    let job = NewJob::new("test.expired", json!({}), 1);
    queue.enqueue(&job).await.unwrap();
    queue
        .claim("crashed-worker", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE background_jobs SET locked_at = NOW() - INTERVAL '31 seconds' WHERE id = $1",
    )
    .bind(job.id)
    .execute(&database.pool)
    .await
    .unwrap();
    assert!(
        queue
            .claim("replacement", Duration::from_secs(30))
            .await
            .unwrap()
            .is_none()
    );
    let state: (String, i32, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, attempts, locked_by, last_error FROM background_jobs WHERE id = $1",
    )
    .bind(job.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        state,
        (
            "dead".to_owned(),
            1,
            None,
            Some("worker lease expired".to_owned())
        )
    );
    database.cleanup().await;
}

#[tokio::test]
async fn claim_reaps_only_a_bounded_batch_of_expired_leases() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let ids: Vec<_> = (0..1_005).map(|_| Uuid::new_v4()).collect();
    sqlx::query(
        "INSERT INTO background_jobs (id, job_type, payload, status, attempts, max_attempts, locked_by, locked_at)
         SELECT id, 'test.reaping', '{}', 'running', 1, 3, 'crashed', NOW() - INTERVAL '1 hour'
         FROM unnest($1::uuid[]) AS id",
    ).bind(&ids).execute(&database.pool).await.unwrap();
    let queue = PostgresJobQueue::new(database.pool.clone());
    queue
        .claim("replacement", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    let expired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM background_jobs WHERE status = 'running' AND locked_by = 'crashed'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(expired, 5);
    queue
        .claim("replacement", Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    let expired: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM background_jobs WHERE status = 'running' AND locked_by = 'crashed'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(expired, 0);
    database.cleanup().await;
}

#[tokio::test]
async fn maintenance_drains_multiple_batches_and_protects_noneligible_jobs() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let ids: Vec<_> = (0..2_505).map(|_| Uuid::new_v4()).collect();
    sqlx::query(
        "INSERT INTO background_jobs (id, job_type, payload, status, attempts, max_attempts, updated_at, completed_at)
         SELECT id, 'test.backlog', '{}', CASE WHEN ordinal % 2 = 0 THEN 'dead' ELSE 'completed' END,
                1, 1, NOW() - INTERVAL '40 days',
                CASE WHEN ordinal % 2 = 0 THEN NULL ELSE NOW() - INTERVAL '40 days' END
         FROM unnest($1::uuid[]) WITH ORDINALITY AS jobs(id, ordinal)",
    ).bind(&ids).execute(&database.pool).await.unwrap();
    let protected: Vec<_> = (0..4).map(|_| Uuid::new_v4()).collect();
    sqlx::query(
        "INSERT INTO background_jobs (id, job_type, payload, status, max_attempts, updated_at, completed_at, locked_by, locked_at)
         VALUES ($1, 'test.protected', '{}', 'completed', 3, NOW(), NOW(), NULL, NULL),
                ($2, 'test.protected', '{}', 'dead', 3, NOW(), NULL, NULL, NULL),
                ($3, 'test.protected', '{}', 'pending', 3, NOW() - INTERVAL '40 days', NULL, NULL, NULL),
                ($4, 'test.protected', '{}', 'running', 3, NOW() - INTERVAL '40 days', NULL, 'active', NOW())",
    ).bind(protected[0]).bind(protected[1]).bind(protected[2]).bind(protected[3])
        .execute(&database.pool).await.unwrap();
    let queue = Arc::new(PostgresJobQueue::new(database.pool.clone()));
    assert_eq!(
        queue
            .purge_terminal(Duration::from_secs(86_400), Duration::from_secs(2_592_000))
            .await
            .unwrap(),
        1_000
    );
    assert_eq!(worker(queue).run_maintenance().await.unwrap(), 1_505);
    let mut remaining: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM background_jobs")
        .fetch_all(&database.pool)
        .await
        .unwrap();
    remaining.sort_unstable();
    let mut protected = protected;
    protected.sort_unstable();
    assert_eq!(remaining, protected);
    database.cleanup().await;
}

struct GatedHandler {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    effects: Arc<AtomicUsize>,
}

#[async_trait]
impl JobHandler for GatedHandler {
    fn job_type(&self) -> &'static str {
        "test.heartbeat"
    }

    async fn handle(&self, _job: &ClaimedJob) -> Result<(), JobHandlerError> {
        self.started.notify_one();
        self.release.notified().await;
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn lease_timestamp(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT locked_at::text FROM background_jobs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn postgres_heartbeat_keeps_a_handler_owned_beyond_its_original_lease() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let queue = Arc::new(PostgresJobQueue::new(database.pool.clone()));
    let job = NewJob::new("test.heartbeat", json!({}), 3);
    queue.enqueue(&job).await.unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let effects = Arc::new(AtomicUsize::new(0));
    let worker = execution_worker(
        queue.clone(),
        vec![Arc::new(GatedHandler {
            started: started.clone(),
            release: release.clone(),
            effects: effects.clone(),
        })],
        Duration::from_secs(3),
    );
    let execution = tokio::spawn(async move { worker.run_once().await });
    started.notified().await;
    let began = tokio::time::Instant::now();
    let mut previous = lease_timestamp(&database.pool, job.id).await;
    // Observe durable renewals rather than assuming a sleep caused a heartbeat.
    for _ in 0..4 {
        previous = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let current = lease_timestamp(&database.pool, job.id).await;
                if current != previous {
                    break current;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker did not renew its PostgreSQL lease");
        assert!(
            queue
                .claim("competitor", Duration::from_secs(3))
                .await
                .unwrap()
                .is_none()
        );
    }
    assert!(began.elapsed() >= Duration::from_secs(3));
    release.notify_one();
    assert_eq!(execution.await.unwrap().unwrap(), RunOutcome::Completed);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let state: (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM background_jobs WHERE id = $1")
            .bind(job.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(state, ("completed".to_owned(), 1));
    database.cleanup().await;
}

#[tokio::test]
async fn persistent_postgres_renewal_outage_cancels_handler_at_lease_expiry() {
    let Some(database) = TestDatabase::new().await else {
        return;
    };
    let queue = Arc::new(PostgresJobQueue::new(database.pool.clone()));
    let job = NewJob::new("test.heartbeat", json!({}), 3);
    queue.enqueue(&job).await.unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let effects = Arc::new(AtomicUsize::new(0));
    let worker = execution_worker(
        queue,
        vec![Arc::new(GatedHandler {
            started: started.clone(),
            release: release.clone(),
            effects: effects.clone(),
        })],
        Duration::from_secs(3),
    );
    let execution = tokio::spawn(async move { worker.run_once().await });
    started.notified().await;
    database.pool.close().await;
    // Renewal errors are retried until the lease budget is exhausted, then the
    // handler is cancelled without any terminal write.
    let began = tokio::time::Instant::now();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(6), execution)
            .await
            .unwrap()
            .unwrap(),
        Err(JobQueueError::LeaseLost),
    );
    assert!(began.elapsed() <= Duration::from_secs(4));
    release.notify_one();
    tokio::task::yield_now().await;
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let state: (String, i32, String) = sqlx::query_as(&format!(
        "SELECT status, attempts, locked_by FROM {}.background_jobs WHERE id = $1",
        database.schema,
    ))
    .bind(job.id)
    .fetch_one(&database.admin)
    .await
    .unwrap();
    assert_eq!(state, ("running".to_owned(), 1, "test-worker".to_owned()));
    database.cleanup().await;
}
