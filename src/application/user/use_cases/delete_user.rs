use std::sync::Arc;

use crate::domain::user::UserId;

use super::super::{ApplicationError, UserCache, UserRepository};

pub struct DeleteUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
}

impl DeleteUserUseCase {
    pub fn new(repository: Arc<dyn UserRepository>, cache: Arc<dyn UserCache>) -> Self {
        Self { repository, cache }
    }

    #[tracing::instrument(name = "application.user.delete", skip(self), fields(user.id = %id))]
    pub async fn execute(&self, id: UserId) -> Result<(), ApplicationError> {
        if !self.repository.delete(id).await? {
            return Err(ApplicationError::NotFound);
        }
        if let Err(e) = self.cache.delete(id).await {
            tracing::warn!(error = %e, user.id = %id, "failed to invalidate cache after user deletion; stale entry will expire via TTL");
        }
        Ok(())
    }
}
