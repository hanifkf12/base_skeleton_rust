use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::application::job::{ClaimedJob, JobDisposition, JobQueue, JobQueueError, NewJob};

pub struct PostgresJobQueue {
    pool: PgPool,
}

impl PostgresJobQueue {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(FromRow)]
struct ClaimedJobRow {
    id: Uuid,
    job_type: String,
    payload: Value,
    trace_context: Value,
    attempts: i32,
    max_attempts: i32,
}

impl ClaimedJobRow {
    fn into_application(self) -> Result<ClaimedJob, JobQueueError> {
        let attempts = u32::try_from(self.attempts).map_err(|error| {
            tracing::error!(%error, job_id = %self.id, "job has invalid attempts");
            JobQueueError::Unavailable
        })?;
        let max_attempts = u32::try_from(self.max_attempts).map_err(|error| {
            tracing::error!(%error, job_id = %self.id, "job has invalid max_attempts");
            JobQueueError::Unavailable
        })?;

        Ok(ClaimedJob {
            id: self.id,
            job_type: self.job_type,
            payload: self.payload,
            trace_context: self.trace_context,
            attempts,
            max_attempts,
        })
    }
}

#[derive(FromRow)]
struct StatusRow {
    status: String,
}

#[async_trait]
impl JobQueue for PostgresJobQueue {
    async fn enqueue(&self, job: &NewJob) -> Result<(), JobQueueError> {
        let max_attempts = max_attempts(job.max_attempts)?;
        sqlx::query(
            r#"INSERT INTO background_jobs (id, job_type, payload, trace_context, max_attempts)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(job.id)
        .bind(&job.job_type)
        .bind(&job.payload)
        .bind(&job.trace_context)
        .bind(max_attempts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn claim(
        &self,
        worker_id: &str,
        lease_timeout: Duration,
    ) -> Result<Option<ClaimedJob>, JobQueueError> {
        let lease_seconds = duration_seconds(lease_timeout);
        let mut transaction = self.pool.begin().await?;

        // Requeue abandoned work. A job whose attempt budget was exhausted is
        // dead-lettered instead, so it cannot remain in `running` forever.
        sqlx::query(
            r#"UPDATE background_jobs
               SET status = CASE
                       WHEN attempts >= max_attempts THEN 'dead'
                       ELSE 'pending'
                   END,
                   available_at = NOW(),
                   locked_at = NULL,
                   locked_by = NULL,
                   last_error = 'worker lease expired',
                   updated_at = NOW()
               WHERE status = 'running'
                 AND locked_at < NOW() - ($1 * INTERVAL '1 second')"#,
        )
        .bind(lease_seconds)
        .execute(&mut *transaction)
        .await?;

        let claimed = sqlx::query_as::<_, ClaimedJobRow>(
            r#"WITH candidate AS (
                   SELECT id
                   FROM background_jobs
                   WHERE status = 'pending'
                     AND available_at <= NOW()
                     AND attempts < max_attempts
                   ORDER BY available_at, created_at
                   FOR UPDATE SKIP LOCKED
                   LIMIT 1
               )
               UPDATE background_jobs AS jobs
               SET status = 'running',
                   attempts = jobs.attempts + 1,
                   locked_at = NOW(),
                   locked_by = $1,
                   updated_at = NOW()
               FROM candidate
               WHERE jobs.id = candidate.id
               RETURNING jobs.id, jobs.job_type, jobs.payload, jobs.trace_context,
                         jobs.attempts, jobs.max_attempts"#,
        )
        .bind(worker_id)
        .fetch_optional(&mut *transaction)
        .await?;

        transaction.commit().await?;
        claimed.map(ClaimedJobRow::into_application).transpose()
    }

    async fn complete(&self, job_id: Uuid, worker_id: &str) -> Result<(), JobQueueError> {
        let result = sqlx::query(
            r#"UPDATE background_jobs
               SET status = 'completed',
                   locked_at = NULL,
                   locked_by = NULL,
                   completed_at = NOW(),
                   updated_at = NOW()
               WHERE id = $1 AND status = 'running' AND locked_by = $2"#,
        )
        .bind(job_id)
        .bind(worker_id)
        .execute(&self.pool)
        .await?;

        ensure_lease(result.rows_affected())
    }

    async fn fail(
        &self,
        job_id: Uuid,
        worker_id: &str,
        error: &str,
        retry_delay: Duration,
    ) -> Result<JobDisposition, JobQueueError> {
        let retry_seconds = duration_seconds(retry_delay);
        let error = error.chars().take(4_000).collect::<String>();
        let row = sqlx::query_as::<_, StatusRow>(
            r#"UPDATE background_jobs
               SET status = CASE
                       WHEN attempts >= max_attempts THEN 'dead'
                       ELSE 'pending'
                   END,
                   available_at = CASE
                       WHEN attempts >= max_attempts THEN available_at
                       ELSE NOW() + ($3 * INTERVAL '1 second')
                   END,
                   locked_at = NULL,
                   locked_by = NULL,
                   last_error = $4,
                   updated_at = NOW()
               WHERE id = $1 AND status = 'running' AND locked_by = $2
               RETURNING status"#,
        )
        .bind(job_id)
        .bind(worker_id)
        .bind(retry_seconds)
        .bind(error)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(JobQueueError::LeaseLost)?;

        match row.status.as_str() {
            "pending" => Ok(JobDisposition::RetryScheduled),
            "dead" => Ok(JobDisposition::DeadLettered),
            status => {
                tracing::error!(%status, %job_id, "job failure returned an invalid status");
                Err(JobQueueError::Unavailable)
            }
        }
    }

    async fn purge_terminal(
        &self,
        completed_older_than: Duration,
        dead_older_than: Duration,
    ) -> Result<u64, JobQueueError> {
        let completed_retention_seconds = duration_seconds(completed_older_than);
        let dead_retention_seconds = duration_seconds(dead_older_than);
        let result = sqlx::query(
            r#"DELETE FROM background_jobs
               WHERE id IN (
                   SELECT id FROM background_jobs
                   WHERE (status = 'completed'
                          AND completed_at < NOW() - ($1 * INTERVAL '1 second'))
                      OR (status = 'dead'
                          AND updated_at < NOW() - ($2 * INTERVAL '1 second'))
                   ORDER BY updated_at
                   LIMIT 1000
               )"#,
        )
        .bind(completed_retention_seconds)
        .bind(dead_retention_seconds)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

fn max_attempts(value: u32) -> Result<i32, JobQueueError> {
    if value == 0 {
        tracing::error!("max_attempts must be greater than zero");
        return Err(JobQueueError::Unavailable);
    }
    i32::try_from(value).map_err(|error| {
        tracing::error!(%error, max_attempts = value, "max_attempts is too large");
        JobQueueError::Unavailable
    })
}

fn duration_seconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

fn ensure_lease(rows_affected: u64) -> Result<(), JobQueueError> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(JobQueueError::LeaseLost)
    }
}

impl From<sqlx::Error> for JobQueueError {
    fn from(error: sqlx::Error) -> Self {
        tracing::error!(error = ?error, "PostgreSQL job queue operation failed");
        JobQueueError::Unavailable
    }
}
