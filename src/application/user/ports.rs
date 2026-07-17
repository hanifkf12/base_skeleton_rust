use async_trait::async_trait;
use thiserror::Error;

use crate::domain::user::{User, UserId};

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryError {
    #[error("the email is already in use")]
    DuplicateEmail,
    #[error("the repository is unavailable")]
    Unavailable,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("the cache is unavailable")]
pub struct CacheError;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: &User) -> Result<User, RepositoryError>;
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError>;
    async fn list(&self, limit: u32, offset: u64) -> Result<Vec<User>, RepositoryError>;
    async fn update(&self, user: &User) -> Result<Option<User>, RepositoryError>;
    async fn delete(&self, id: UserId) -> Result<bool, RepositoryError>;
}

/// Cache misses are `Ok(None)`. All TTL values are whole seconds.
#[async_trait]
pub trait UserCache: Send + Sync {
    async fn get(&self, id: UserId) -> Result<Option<User>, CacheError>;
    async fn set(&self, user: &User, ttl_seconds: u64) -> Result<(), CacheError>;
    async fn delete(&self, id: UserId) -> Result<(), CacheError>;
}
