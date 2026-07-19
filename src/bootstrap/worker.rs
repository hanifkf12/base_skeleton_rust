use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    application::job::{JobHandler, JobQueue, JobWorker, RunOutcome},
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
    let worker = JobWorker::new(
        queue,
        handlers,
        tracer,
        worker_id.clone(),
        Duration::from_secs(config.job_lease_timeout_seconds),
        Duration::from_secs(config.job_retry_base_seconds),
        Duration::from_secs(config.job_retry_max_seconds),
        Duration::from_secs(config.job_completed_retention_seconds),
    );

    tracing::info!(%worker_id, "PostgreSQL job worker started");

    loop {
        if shutdown::requested(&shutdown_receiver) {
            break;
        }

        // Finish an active job before observing shutdown. Handlers still need
        // idempotency because at-least-once delivery permits crash recovery.
        let should_pause = match worker.run_once().await {
            Ok(RunOutcome::Idle) => true,
            Ok(_) => false,
            Err(error) => {
                tracing::error!(%error, "job worker iteration failed");
                true
            }
        };

        if should_pause && shutdown::wait_or_timeout(&mut shutdown_receiver, poll_interval).await {
            break;
        }
    }

    tracing::info!(%worker_id, "PostgreSQL job worker stopped");
    Ok(())
}
