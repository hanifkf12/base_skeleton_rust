use std::sync::Arc;

use crate::domain::user::{DisplayName, Email, User, UserId};

use super::super::{ApplicationError, UpdateUserInput, UserCache, UserRepository};

pub struct UpdateUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
    cache_ttl_seconds: u64,
}

impl UpdateUserUseCase {
    pub fn new(
        repository: Arc<dyn UserRepository>,
        cache: Arc<dyn UserCache>,
        cache_ttl_seconds: u64,
    ) -> Self {
        Self {
            repository,
            cache,
            cache_ttl_seconds,
        }
    }

    #[tracing::instrument(name = "application.user.update", skip(self, input), fields(user.id = %id))]
    pub async fn execute(
        &self,
        id: UserId,
        input: UpdateUserInput,
    ) -> Result<User, ApplicationError> {
        let mut user = self
            .repository
            .find_by_id(id)
            .await?
            .ok_or(ApplicationError::NotFound)?;
        user.update_profile(
            Email::parse(input.email)?,
            DisplayName::parse(input.display_name)?,
        );
        let updated = self
            .repository
            .update(&user)
            .await?
            .ok_or(ApplicationError::NotFound)?;
        let _ = self.cache.set(&updated, self.cache_ttl_seconds).await;
        Ok(updated)
    }
}
