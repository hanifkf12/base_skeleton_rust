use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    application::job::{JobHandler, JobQueue, JobWorker, JobWorkerConfig, RunOutcome},
    config::Config,
    infrastructure::{database::postgres::PostgresJobQueue, job::UserCreatedHandler},
    telemetry::OpenTelemetryJobTracer,
};

use super::shutdown;

pub async fn run(config: Config, mut shutdown_receiver: watch::Receiver<bool>) -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
        .context("could not connect worker to PostgreSQL")?;

    let queue: Arc<dyn JobQueue> = Arc::new(PostgresJobQueue::new(pool));
    let handlers: Vec<Arc<dyn JobHandler>> = vec![Arc::new(UserCreatedHandler)];
    let tracer = Arc::new(OpenTelemetryJobTracer);
    let worker_id = config
        .job_worker_id
        .unwrap_or_else(|| format!("worker-{}", Uuid::new_v4()));
    let poll_interval = Duration::from_millis(config.job_poll_interval_milliseconds);
    let cleanup_interval = Duration::from_secs(config.job_cleanup_interval_seconds);
    let worker = JobWorker::new(
        queue,
        handlers,
        tracer,
        JobWorkerConfig {
            worker_id: worker_id.clone(),
            lease_timeout: Duration::from_secs(config.job_lease_timeout_seconds),
            retry_base: Duration::from_secs(config.job_retry_base_seconds),
            retry_max: Duration::from_secs(config.job_retry_max_seconds),
            completed_retention: Duration::from_secs(config.job_completed_retention_seconds),
            dead_retention: Duration::from_secs(config.job_dead_retention_seconds),
        },
    );
    let mut next_cleanup = tokio::time::Instant::now();

    tracing::info!(%worker_id, "PostgreSQL job worker started");

    loop {
        if shutdown::requested(&shutdown_receiver) {
            break;
        }

        if tokio::time::Instant::now() >= next_cleanup {
            // Cleanup can stop between batches; an active handler below still
            // finishes before shutdown is observed.
            tokio::select! {
                _ = shutdown::wait(&mut shutdown_receiver) => break,
                _ = cleanup(&worker) => {}
            }
            next_cleanup = tokio::time::Instant::now() + cleanup_interval;
        }

        // Finish an active job before observing shutdown. Handlers still need
        // idempotency because at-least-once delivery permits crash recovery.
        let should_pause = iteration(&worker).await;

        if should_pause && shutdown::wait_or_timeout(&mut shutdown_receiver, poll_interval).await {
            break;
        }
    }

    tracing::info!(%worker_id, "PostgreSQL job worker stopped");
    Ok(())
}

async fn cleanup(worker: &JobWorker) {
    match worker.run_maintenance().await {
        Ok(purged) if purged > 0 => {
            crate::telemetry::record_cleanup_count(purged);
            tracing::info!(purged_jobs = purged, "purged terminal jobs");
        }
        Ok(_) => {}
        Err(error) => {
            crate::telemetry::record_worker_error("cleanup");
            tracing::error!(%error, "job cleanup failed");
        }
    }
}

async fn iteration(worker: &JobWorker) -> bool {
    match worker.run_once().await {
        Ok(RunOutcome::Idle) => true,
        Ok(_) => false,
        Err(error) => {
            crate::telemetry::record_worker_error("iteration");
            tracing::error!(%error, "job worker iteration failed");
            true
        }
    }
}
