use std::{collections::HashMap, sync::Arc, time::Duration};

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

        let result = async {
            match self.handlers.get(job.job_type.as_str()) {
                Some(handler) => handler.handle(&job).await,
                None => Err(super::JobHandlerError::new(format!(
                    "no handler is registered for job type {}",
                    job.job_type
                ))),
            }
        }
        .instrument(self.tracer.span(&job))
        .await;

        match result {
            Ok(()) => {
                self.queue.complete(job.id, &self.worker_id).await?;
                tracing::info!(job_id = %job.id, job_type = %job.job_type, "job completed");
                if let Ok(purged) = self.queue.purge_completed(self.completed_retention).await
                    && purged > 0
                {
                    tracing::debug!(purged_jobs = purged, "purged completed jobs");
                }
                Ok(RunOutcome::Completed)
            }
            Err(error) => {
                let delay = retry_delay(self.retry_base, self.retry_max, job.attempts);
                let disposition = self
                    .queue
                    .fail(job.id, &self.worker_id, &error.to_string(), delay)
                    .await?;

                tracing::warn!(
                    job_id = %job.id,
                    job_type = %job.job_type,
                    attempt = job.attempts,
                    max_attempts = job.max_attempts,
                    %error,
                    ?disposition,
                    "job failed"
                );

                Ok(match disposition {
                    JobDisposition::RetryScheduled => RunOutcome::RetryScheduled,
                    JobDisposition::DeadLettered => RunOutcome::DeadLettered,
                })
            }
        }
    }
}

fn retry_delay(base: Duration, maximum: Duration, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(31);
    base.saturating_mul(1_u32 << exponent).min(maximum)
}
