use std::sync::Arc;

use crate::domain::user::{DisplayName, Email, User};

use super::super::{ApplicationError, CreateUserInput, UserCache, UserRepository};

pub struct CreateUserUseCase {
    repository: Arc<dyn UserRepository>,
    cache: Arc<dyn UserCache>,
    cache_ttl_seconds: u64,
}

impl CreateUserUseCase {
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

    pub async fn execute(&self, input: CreateUserInput) -> Result<User, ApplicationError> {
        let user = User::new(
            Email::parse(input.email)?,
            DisplayName::parse(input.display_name)?,
        );
        let created = self.repository.create(&user).await?;
        let _ = self.cache.set(&created, self.cache_ttl_seconds).await;
        Ok(created)
    }
}
