use std::sync::Arc;

use crate::domain::user::{User, UserId};

use super::super::{ApplicationError, UserCache, UserRepository};

pub struct GetUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
    cache_ttl_seconds: u64,
}

impl GetUserUseCase {
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

    #[tracing::instrument(name = "application.user.get", skip(self), fields(user.id = %id))]
    pub async fn execute(&self, id: UserId) -> Result<User, ApplicationError> {
        if let Ok(Some(user)) = self.cache.get(id).await {
            return Ok(user);
        }

        let user = self
            .repository
            .find_by_id(id)
            .await?
            .ok_or(ApplicationError::NotFound)?;
        let _ = self.cache.set(&user, self.cache_ttl_seconds).await;
        Ok(user)
    }
}
