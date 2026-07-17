use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::{
    application::job::{JobHandler, JobQueue, JobWorker, RunOutcome},
    config::Config,
    infrastructure::{database::postgres::PostgresJobQueue, job::UserCreatedHandler},
    telemetry,
};

use super::app::shutdown_signal;

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    telemetry::init();

    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
        .context("could not connect worker to PostgreSQL")?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("could not run PostgreSQL migrations")?;

    let queue: Arc<dyn JobQueue> = Arc::new(PostgresJobQueue::new(pool));
    let handlers: Vec<Arc<dyn JobHandler>> = vec![Arc::new(UserCreatedHandler)];
    let worker_id = config
        .job_worker_id
        .unwrap_or_else(|| format!("worker-{}", Uuid::new_v4()));
    let poll_interval = Duration::from_millis(config.job_poll_interval_milliseconds);
    let worker = JobWorker::new(
        queue,
        handlers,
        worker_id.clone(),
        Duration::from_secs(config.job_lease_timeout_seconds),
        Duration::from_secs(config.job_retry_base_seconds),
        Duration::from_secs(config.job_retry_max_seconds),
    );

    tracing::info!(%worker_id, "PostgreSQL job worker started");
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        let should_pause = tokio::select! {
            () = &mut shutdown => break,
            result = worker.run_once() => match result {
                Ok(RunOutcome::Idle) => true,
                Ok(_) => false,
                Err(error) => {
                    tracing::error!(%error, "job worker iteration failed");
                    true
                }
            }
        };

        if should_pause {
            tokio::select! {
                () = &mut shutdown => break,
                () = tokio::time::sleep(poll_interval) => {}
            }
        }
    }

    tracing::info!(%worker_id, "PostgreSQL job worker stopped");
    Ok(())
}
