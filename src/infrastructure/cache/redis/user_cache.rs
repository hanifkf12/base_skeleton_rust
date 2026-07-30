use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, aio::ConnectionManager};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    application::user::{CacheError, UserCache},
    domain::user::{DisplayName, Email, User, UserId},
};

pub struct RedisUserCache {
    connection: ConnectionManager,
}

impl RedisUserCache {
    pub fn new(connection: ConnectionManager) -> Self {
        Self { connection }
    }

    fn key(id: UserId) -> String {
        format!("users:{id}")
    }
}

#[derive(Serialize, Deserialize)]
struct CachedUser {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<&User> for CachedUser {
    fn from(user: &User) -> Self {
        Self {
            id: *user.id().as_uuid(),
            email: user.email().as_str().to_owned(),
            display_name: user.display_name().as_str().to_owned(),
            created_at: *user.created_at(),
            updated_at: *user.updated_at(),
        }
    }
}

impl TryFrom<CachedUser> for User {
    type Error = CacheError;

    fn try_from(value: CachedUser) -> Result<Self, Self::Error> {
        Ok(User::restore(
            UserId::from_uuid(value.id),
            Email::parse(value.email).map_err(|e| CacheError::Unavailable(e.to_string()))?,
            DisplayName::parse(value.display_name)
                .map_err(|e| CacheError::Unavailable(e.to_string()))?,
            value.created_at,
            value.updated_at,
        ))
    }
}

#[async_trait]
impl UserCache for RedisUserCache {
    #[tracing::instrument(name = "infrastructure.redis.user_cache.get", skip(self), fields(db.system = "redis", user.id = %id))]
    async fn get(&self, id: UserId) -> Result<Option<User>, CacheError> {
        let mut connection = self.connection.clone();
        let value: Option<String> = connection
            .get(Self::key(id))
            .await
            .map_err(CacheError::from)?;
        value
            .map(|json| {
                serde_json::from_str::<CachedUser>(&json)
                    .map_err(CacheError::from)?
                    .try_into()
            })
            .transpose()
    }

    #[tracing::instrument(name = "infrastructure.redis.user_cache.set", skip(self, user), fields(db.system = "redis", user.id = %user.id(), cache.ttl_seconds = ttl_seconds))]
    async fn set(&self, user: &User, ttl_seconds: u64) -> Result<(), CacheError> {
        let json = serde_json::to_string(&CachedUser::from(user)).map_err(CacheError::from)?;
        let mut connection = self.connection.clone();
        connection
            .set_ex::<_, _, ()>(Self::key(user.id()), json, ttl_seconds)
            .await
            .map_err(CacheError::from)
    }

    #[tracing::instrument(name = "infrastructure.redis.user_cache.delete", skip(self), fields(db.system = "redis", user.id = %id))]
    async fn delete(&self, id: UserId) -> Result<(), CacheError> {
        let mut connection = self.connection.clone();
        connection
            .del::<_, ()>(Self::key(id))
            .await
            .map_err(CacheError::from)
    }
}

impl From<redis::RedisError> for CacheError {
    fn from(error: redis::RedisError) -> Self {
        tracing::warn!(error = ?error, "Redis user cache operation failed");
        CacheError::Unavailable(error.to_string())
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(error: serde_json::Error) -> Self {
        tracing::warn!(error = ?error, "Redis user cache serialization failed");
        CacheError::Serialization(error.to_string())
    }
}
