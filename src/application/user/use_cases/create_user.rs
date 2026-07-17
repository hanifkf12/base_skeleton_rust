use std::sync::Arc;

use serde_json::json;

use crate::{
    application::job::{NewJob, USER_CREATED_JOB},
    domain::user::{DisplayName, Email, User},
};

use super::super::{ApplicationError, CreateUserInput, UserCache, UserRegistrationRepository};

pub struct CreateUserUseCase {
    repository: Arc<dyn UserRegistrationRepository>,
    cache: Arc<dyn UserCache>,
    cache_ttl_seconds: u64,
    job_max_attempts: u32,
}

impl CreateUserUseCase {
    pub fn new(
        repository: Arc<dyn UserRegistrationRepository>,
        cache: Arc<dyn UserCache>,
        cache_ttl_seconds: u64,
        job_max_attempts: u32,
    ) -> Self {
        Self {
            repository,
            cache,
            cache_ttl_seconds,
            job_max_attempts,
        }
    }

    pub async fn execute(&self, input: CreateUserInput) -> Result<User, ApplicationError> {
        let user = User::new(
            Email::parse(input.email)?,
            DisplayName::parse(input.display_name)?,
        );
        let job = NewJob::new(
            USER_CREATED_JOB,
            json!({
                "user_id": user.id().to_string(),
            }),
            self.job_max_attempts,
        )
        .with_trace_context(crate::telemetry::current_trace_context());
        let created = self.repository.create_with_job(&user, &job).await?;
        let _ = self.cache.set(&created, self.cache_ttl_seconds).await;
        Ok(created)
    }
}
