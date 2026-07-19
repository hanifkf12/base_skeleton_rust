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

    async fn complete(&self, job_id: Uuid, worker_id: &str) -> Result<(), JobQueueError>;

    async fn fail(
        &self,
        job_id: Uuid,
        worker_id: &str,
        error: &str,
        retry_delay: Duration,
    ) -> Result<JobDisposition, JobQueueError>;

    async fn purge_completed(&self, older_than: Duration) -> Result<u64, JobQueueError>;
}

#[async_trait]
pub trait JobHandler: Send + Sync {
    fn job_type(&self) -> &'static str;
    async fn handle(&self, job: &ClaimedJob) -> Result<(), JobHandlerError>;
}

pub trait JobTracer: Send + Sync {
    fn span(&self, job: &ClaimedJob) -> Span;
}
