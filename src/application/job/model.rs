use serde_json::Value;
use uuid::Uuid;

pub const USER_CREATED_JOB: &str = "user.created";

#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: Uuid,
    pub job_type: String,
    pub payload: Value,
    pub max_attempts: u32,
}

impl NewJob {
    pub fn new(job_type: impl Into<String>, payload: Value, max_attempts: u32) -> Self {
        Self {
            id: Uuid::new_v4(),
            job_type: job_type.into(),
            payload,
            max_attempts,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub job_type: String,
    pub payload: Value,
    /// The current attempt, starting at one. Claiming a job increments this value.
    pub attempts: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobDisposition {
    RetryScheduled,
    DeadLettered,
}
