use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use tracing::Instrument;

use super::{
    ClaimedJob, JobDisposition, JobHandler, JobHandlerError, JobQueue, JobQueueError, JobTracer,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Idle,
    Completed,
    RetryScheduled,
    DeadLettered,
}

pub struct JobWorkerConfig {
    pub worker_id: String,
    pub lease_timeout: Duration,
    pub retry_base: Duration,
    pub retry_max: Duration,
    pub completed_retention: Duration,
    pub dead_retention: Duration,
}

pub struct JobWorker {
    queue: Arc<dyn JobQueue>,
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
    tracer: Arc<dyn JobTracer>,
    config: JobWorkerConfig,
}

impl JobWorker {
    pub fn new(
        queue: Arc<dyn JobQueue>,
        handlers: Vec<Arc<dyn JobHandler>>,
        tracer: Arc<dyn JobTracer>,
        config: JobWorkerConfig,
    ) -> Self {
        let handlers = handlers
            .into_iter()
            .map(|handler| (handler.job_type(), handler))
            .collect();

        Self {
            queue,
            handlers,
            tracer,
            config,
        }
    }

    pub async fn run_once(&self) -> Result<RunOutcome, JobQueueError> {
        // Budget from before the claim: database/network latency must not extend
        // our estimate of ownership beyond the lease stored in PostgreSQL.
        let claimed_at = tokio::time::Instant::now();
        let Some(job) = self
            .queue
            .claim(&self.config.worker_id, self.config.lease_timeout)
            .await?
        else {
            return Ok(RunOutcome::Idle);
        };
        let started = Instant::now();

        let span = self.tracer.span(&job);
        let (result, deadline) = self
            .execute_with_heartbeat(&job, claimed_at)
            .instrument(span.clone())
            .await
            .inspect_err(|_| {
                span.record("otel.status_code", "ERROR");
                span.record("otel.status_description", "lease_renewal_failed");
            })?;

        if tokio::time::Instant::now() >= deadline {
            return Err(JobQueueError::LeaseLost);
        }

        match result {
            Ok(()) => {
                let completion = self
                    .queue
                    .complete(job.id, &self.config.worker_id, job.attempts);
                if let Err(error) = tokio::time::timeout_at(deadline, completion)
                    .await
                    .map_err(|_| JobQueueError::LeaseLost)?
                {
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
                let delay =
                    retry_delay(self.config.retry_base, self.config.retry_max, job.attempts);
                let message = error.to_string();
                let failure = self.queue.fail(
                    job.id,
                    &self.config.worker_id,
                    job.attempts,
                    &message,
                    delay,
                );
                let disposition = match tokio::time::timeout_at(deadline, failure)
                    .await
                    .map_err(|_| JobQueueError::LeaseLost)?
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

                let (outcome, label) = match disposition {
                    JobDisposition::RetryScheduled => {
                        (RunOutcome::RetryScheduled, "retry_scheduled")
                    }
                    JobDisposition::DeadLettered => (RunOutcome::DeadLettered, "dead_lettered"),
                };
                crate::telemetry::record_job_outcome(&job.job_type, label, started.elapsed());
                Ok(outcome)
            }
        }
    }

    async fn execute_with_heartbeat(
        &self,
        job: &ClaimedJob,
        claimed_at: tokio::time::Instant,
    ) -> Result<(Result<(), JobHandlerError>, tokio::time::Instant), JobQueueError> {
        let handler = async {
            match self.handlers.get(job.job_type.as_str()) {
                Some(handler) => handler.handle(job).await,
                None => Err(JobHandlerError::new(format!(
                    "no handler is registered for job type {}",
                    job.job_type
                ))),
            }
        };
        tokio::pin!(handler);
        let interval = (self.config.lease_timeout / 3).max(Duration::from_nanos(1));
        let mut deadline = claimed_at + self.config.lease_timeout;
        let mut next_renewal = claimed_at + interval;

        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(JobQueueError::LeaseLost);
            }
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(next_renewal) => {}
                result = tokio::time::timeout_at(deadline, &mut handler) => {
                    return Ok((result.map_err(|_| JobQueueError::LeaseLost)?, deadline));
                }
            }

            let renewal_started = tokio::time::Instant::now();
            if renewal_started >= deadline {
                return Err(JobQueueError::LeaseLost);
            }
            let renewal = tokio::time::timeout_at(
                deadline,
                self.queue
                    .renew(job.id, &self.config.worker_id, job.attempts),
            );
            tokio::pin!(renewal);
            // Continue polling the handler while the database renews. If it
            // finishes during renewal, establish ownership before any terminal write.
            let finished = tokio::select! {
                biased;
                result = &mut renewal => {
                    result.map_err(|_| JobQueueError::LeaseLost)??;
                    None
                }
                result = &mut handler => {
                    renewal.await.map_err(|_| JobQueueError::LeaseLost)??;
                    Some(result)
                }
            };
            deadline = renewal_started + self.config.lease_timeout;
            next_renewal = renewal_started + interval;
            if let Some(result) = finished {
                return Ok((result, deadline));
            }
        }
    }

    pub async fn run_maintenance(&self) -> Result<u64, JobQueueError> {
        // Each adapter statement deletes at most 1,000 rows; a finite cycle
        // drains ordinary backlogs without monopolizing a worker indefinitely.
        let mut total = 0;
        for _ in 0..16 {
            let purged = self
                .queue
                .purge_terminal(self.config.completed_retention, self.config.dead_retention)
                .await?;
            total += purged;
            if purged < 1_000 {
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok(total)
    }
}

fn retry_delay(base: Duration, maximum: Duration, attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(31);
    base.saturating_mul(1_u32 << exponent).min(maximum)
}
