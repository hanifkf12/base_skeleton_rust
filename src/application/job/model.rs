use serde_json::{Value, json};
use uuid::Uuid;

pub const USER_CREATED_JOB: &str = "user.created";

#[derive(Debug, Clone)]
pub struct NewJob {
    pub id: Uuid,
    pub job_type: String,
    pub payload: Value,
    /// W3C trace context captured when the job was produced. It is opaque to
    /// application code and lets the worker continue the originating trace.
    pub trace_context: Value,
    pub max_attempts: u32,
}

impl NewJob {
    pub fn new(job_type: impl Into<String>, payload: Value, max_attempts: u32) -> Self {
        Self {
            id: Uuid::new_v4(),
            job_type: job_type.into(),
            payload,
            trace_context: json!({}),
            max_attempts,
        }
    }

    pub fn with_trace_context(mut self, trace_context: Value) -> Self {
        self.trace_context = trace_context;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub job_type: String,
    pub payload: Value,
    pub trace_context: Value,
    /// The current attempt, starting at one. Claiming a job increments this value.
    pub attempts: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobDisposition {
    RetryScheduled,
    DeadLettered,
}
