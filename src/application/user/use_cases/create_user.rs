use std::sync::Arc;

use serde_json::json;

use crate::{
    application::job::{NewJob, USER_CREATED_JOB},
    domain::user::{DisplayName, Email, User},
};

use super::super::{
    ApplicationError, CreateUserInput, TraceContextProvider, UserCache, UserRegistrationRepository,
};

pub struct CreateUserUseCase {
    repository: Arc<dyn UserRegistrationRepository>,
    cache: Arc<dyn UserCache>,
    trace_context: Arc<dyn TraceContextProvider>,
    cache_ttl_seconds: u64,
    job_max_attempts: u32,
}

impl CreateUserUseCase {
    pub fn new(
        repository: Arc<dyn UserRegistrationRepository>,
        cache: Arc<dyn UserCache>,
        trace_context: Arc<dyn TraceContextProvider>,
        cache_ttl_seconds: u64,
        job_max_attempts: u32,
    ) -> Self {
        Self {
            repository,
            cache,
            trace_context,
            cache_ttl_seconds,
            job_max_attempts,
        }
    }

    #[tracing::instrument(name = "application.user.create", skip(self, input))]
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
        .with_trace_context(self.trace_context.current());
        let created = self.repository.create_with_job(&user, &job).await?;
        if let Err(e) = self.cache.set(&created, self.cache_ttl_seconds).await {
            tracing::warn!(error = %e, user.id = %created.id(), "failed to write newly created user to cache");
        }
        Ok(created)
    }
}
