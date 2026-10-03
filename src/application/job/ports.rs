use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tracing::Span;
use uuid::Uuid;

use super::{ClaimedJob, JobDisposition, NewJob};

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum JobQueueError {
    #[error("the job queue is unavailable")]
    Unavailable,
    #[error("the job lease is no longer owned by this worker")]
    LeaseLost,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{message}")]
pub struct JobHandlerError {
    message: String,
}

impl JobHandlerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, job: &NewJob) -> Result<(), JobQueueError>;

    async fn claim(
        &self,
        worker_id: &str,
        lease_timeout: Duration,
    ) -> Result<Option<ClaimedJob>, JobQueueError>;

    async fn complete(
        &self,
        job_id: Uuid,
        worker_id: &str,
        attempt: u32,
    ) -> Result<(), JobQueueError>;

    async fn renew(&self, job_id: Uuid, worker_id: &str, attempt: u32)
    -> Result<(), JobQueueError>;

    async fn fail(
        &self,
        job_id: Uuid,
        worker_id: &str,
        attempt: u32,
        error: &str,
        retry_delay: Duration,
    ) -> Result<JobDisposition, JobQueueError>;

    /// Delete at most 1,000 eligible terminal jobs per call. Workers may call
    /// repeatedly within a finite maintenance cycle to drain larger backlogs.
    async fn purge_terminal(
        &self,
        completed_older_than: Duration,
        dead_older_than: Duration,
    ) -> Result<u64, JobQueueError>;
}

#[async_trait]
/// Jobs are delivered at least once. Implementations must make external side effects
/// idempotent: lease loss cancels the handler future, but cannot undo effects already
/// committed (or work spawned independently of that future).
pub trait JobHandler: Send + Sync {
    fn job_type(&self) -> &'static str;
    async fn handle(&self, job: &ClaimedJob) -> Result<(), JobHandlerError>;
}

pub trait JobTracer: Send + Sync {
    fn span(&self, job: &ClaimedJob) -> Span;
}
