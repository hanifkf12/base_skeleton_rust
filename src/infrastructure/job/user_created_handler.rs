use async_trait::async_trait;
use serde::Deserialize;
use uuid::Uuid;

use crate::application::job::{ClaimedJob, JobHandler, JobHandlerError, USER_CREATED_JOB};

/// Example handler proving the complete queue path. Replace or extend its body
/// with the real side effect (email, webhook, audit write, and so on).
pub struct UserCreatedHandler;

#[derive(Deserialize)]
struct UserCreatedPayload {
    user_id: Uuid,
}

#[async_trait]
impl JobHandler for UserCreatedHandler {
    fn job_type(&self) -> &'static str {
        USER_CREATED_JOB
    }

    async fn handle(&self, job: &ClaimedJob) -> Result<(), JobHandlerError> {
        let payload: UserCreatedPayload =
            serde_json::from_value(job.payload.clone()).map_err(|error| {
                JobHandlerError::new(format!("invalid user.created payload: {error}"))
            })?;

        tracing::info!(
            job_id = %job.id,
            user_id = %payload.user_id,
            "processed user.created job"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn job(payload: serde_json::Value) -> ClaimedJob {
        ClaimedJob {
            id: Uuid::new_v4(),
            job_type: USER_CREATED_JOB.to_owned(),
            payload,
            attempts: 1,
            max_attempts: 5,
        }
    }

    #[tokio::test]
    async fn accepts_a_valid_payload() {
        let result = UserCreatedHandler
            .handle(&job(json!({ "user_id": Uuid::new_v4() })))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rejects_an_invalid_payload() {
        let result = UserCreatedHandler
            .handle(&job(json!({ "user_id": "invalid" })))
            .await;

        assert!(result.is_err());
    }
}
