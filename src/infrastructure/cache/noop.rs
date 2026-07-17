use async_trait::async_trait;

use crate::{
    application::user::{CacheError, UserCache},
    domain::user::{User, UserId},
};

/// Cache adapter used when no cache service is configured or available.
pub struct NoOpUserCache;

#[async_trait]
impl UserCache for NoOpUserCache {
    async fn get(&self, _id: UserId) -> Result<Option<User>, CacheError> {
        Ok(None)
    }

    async fn set(&self, _user: &User, _ttl_seconds: u64) -> Result<(), CacheError> {
        Ok(())
    }

    async fn delete(&self, _id: UserId) -> Result<(), CacheError> {
        Ok(())
    }
}
