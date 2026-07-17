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

    pub async fn execute(&self, id: UserId) -> Result<(), ApplicationError> {
        if !self.repository.delete(id).await? {
            return Err(ApplicationError::NotFound);
        }
        let _ = self.cache.delete(id).await;
        Ok(())
    }
}
