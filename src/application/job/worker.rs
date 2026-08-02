use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use tracing::Instrument;

use super::{JobDisposition, JobHandler, JobQueue, JobQueueError, JobTracer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Idle,
    Completed,
    RetryScheduled,
    DeadLettered,
}

pub struct JobWorker {
    queue: Arc<dyn JobQueue>,
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
    tracer: Arc<dyn JobTracer>,
    worker_id: String,
    lease_timeout: Duration,
    retry_base: Duration,
    retry_max: Duration,
    completed_retention: Duration,
    dead_retention: Duration,
}

impl JobWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        queue: Arc<dyn JobQueue>,
        handlers: Vec<Arc<dyn JobHandler>>,
        tracer: Arc<dyn JobTracer>,
        worker_id: String,
        lease_timeout: Duration,
        retry_base: Duration,
        retry_max: Duration,
        completed_retention: Duration,
        dead_retention: Duration,
    ) -> Self {
        let handlers = handlers
            .into_iter()
            .map(|handler| (handler.job_type(), handler))
            .collect();

        Self {
            queue,
            handlers,
            tracer,
            worker_id,
            lease_timeout,
            retry_base,
            retry_max,
            completed_retention,
            dead_retention,
        }
    }

    pub async fn run_once(&self) -> Result<RunOutcome, JobQueueError> {
        let Some(job) = self
            .queue
            .claim(&self.worker_id, self.lease_timeout)
            .await?
        else {
            return Ok(RunOutcome::Idle);
        };
        let started = Instant::now();

        let span = self.tracer.span(&job);
        let result = async {
            match self.handlers.get(job.job_type.as_str()) {
                Some(handler) => handler.handle(&job).await,
                None => Err(super::JobHandlerError::new(format!(
                    "no handler is registered for job type {}",
                    job.job_type
                ))),
            }
        }
        .instrument(span.clone())
        .await;

        match result {
            Ok(()) => {
                if let Err(error) = self.queue.complete(job.id, &self.worker_id).await {
                    span.record("otel.status_code", "ERROR");
                    span.record("otel.status_description", "complete_failed");
                    return Err(error);
                }
                crate::telemetry::record_job_outcome(&job.job_type, "completed", started.elapsed());
                tracing::info!(job_id = %job.id, job_type = %job.job_type, "job completed");
                Ok(RunOutcome::Completed)
            }
            Err(error) => {
                span.record("otel.status_code", "ERROR");
                span.record("otel.status_description", "handler_failed");
                let delay = retry_delay(self.retry_base, self.retry_max, job.attempts);
                let disposition = match self
                    .queue
                    .fail(job.id, &self.worker_id, &error.to_string(), delay)
                    .await
                {
                    Ok(disposition) => disposition,
                    Err(queue_error) => {
                        span.record("otel.status_description", "fail_failed");
                        return Err(queue_error);
                    }
                };

                tracing::warn!(
                    job_id = %job.id,
                    job_type = %job.job_type,
                    attempt = job.attempts,
                    max_attempts = job.max_attempts,
                    %error,
                    ?disposition,
                    "job failed"
                );

                let outcome = match disposition {
                    JobDisposition::RetryScheduled => RunOutcome::RetryScheduled,
                    JobDisposition::DeadLettered => RunOutcome::DeadLettered,
                };
                crate::telemetry::record_job_outcome(
                    &job.job_type,
                    match outcome {
                        RunOutcome::RetryScheduled => "retry_scheduled",
                        RunOutcome::DeadLettered => "dead_lettered",
                        _ => unreachable!("failure disposition only produces failure outcomes"),
                    },
                    started.elapsed(),
                );
                Ok(outcome)
            }
        }
    }

    pub async fn run_maintenance(&self) -> Result<u64, JobQueueError> {
        self.queue
            .purge_terminal(self.completed_retention, self.dead_retention)
            .await
    }
}

fn retry_delay(base: Duration, maximum: Duration, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(31);
    base.saturating_mul(1_u32 << exponent).min(maximum)
}
