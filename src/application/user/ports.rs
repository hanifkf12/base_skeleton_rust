use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use thiserror::Error;

use crate::{
    application::job::NewJob,
    domain::user::{User, UserId},
};

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryError {
    #[error("the email is already in use")]
    DuplicateEmail,
    #[error("the repository is unavailable")]
    Unavailable,
    #[error("the entity was modified by another operation")]
    Conflict,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("the cache is unavailable")]
pub struct CacheError;

#[async_trait]
pub trait UserRepository: Send + Sync {
    async fn create(&self, user: &User) -> Result<User, RepositoryError>;
    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError>;
    async fn list(&self, limit: u32, offset: u64) -> Result<Vec<User>, RepositoryError>;
    async fn update(
        &self,
        user: &User,
        expected_updated_at: &DateTime<Utc>,
    ) -> Result<Option<User>, RepositoryError>;
    async fn delete(&self, id: UserId) -> Result<bool, RepositoryError>;
}

/// Persists a new user and its first background job atomically.
#[async_trait]
pub trait UserRegistrationRepository: Send + Sync {
    async fn create_with_job(&self, user: &User, job: &NewJob) -> Result<User, RepositoryError>;
}

/// Cache misses are `Ok(None)`. All TTL values are whole seconds.
#[async_trait]
pub trait UserCache: Send + Sync {
    async fn get(&self, id: UserId) -> Result<Option<User>, CacheError>;
    async fn set(&self, user: &User, ttl_seconds: u64) -> Result<(), CacheError>;
    async fn delete(&self, id: UserId) -> Result<(), CacheError>;
}

pub trait TraceContextProvider: Send + Sync {
    fn current(&self) -> Value;
}
