use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use redis::Client;
use sqlx::postgres::PgPoolOptions;

use crate::{
    application::health::ReadinessCheck,
    application::user::{
        CreateUserUseCase, DeleteUserUseCase, GetUserUseCase, ListUsersUseCase, UpdateUserUseCase,
        UserCache, UserRegistrationRepository, UserRepository,
    },
    config::Config,
    infrastructure::{
        cache::{noop::NoOpUserCache, redis::RedisUserCache},
        database::postgres::{PostgresReadinessCheck, PostgresUserRepository},
    },
    presentation::http::AppState,
};

pub struct Dependencies {
    pub state: AppState,
    pub readiness: Arc<dyn ReadinessCheck>,
}

pub async fn build_dependencies(config: &Config) -> Result<Dependencies> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
        .context("could not connect to PostgreSQL")?;

    let readiness: Arc<dyn ReadinessCheck> = Arc::new(PostgresReadinessCheck::new(pool.clone()));
    let postgres_repository = Arc::new(PostgresUserRepository::new(pool));
    let registration_repository: Arc<dyn UserRegistrationRepository> = postgres_repository.clone();
    let repository: Arc<dyn UserRepository> = postgres_repository;
    let cache = build_cache(config).await?;
    let ttl = config.user_cache_ttl_seconds;

    let state = AppState {
        create_user: Arc::new(CreateUserUseCase::new(
            registration_repository,
            cache.clone(),
            ttl,
            config.job_max_attempts,
        )),
        get_user: Arc::new(GetUserUseCase::new(repository.clone(), cache.clone(), ttl)),
        list_users: Arc::new(ListUsersUseCase::new(repository.clone())),
        update_user: Arc::new(UpdateUserUseCase::new(
            repository.clone(),
            cache.clone(),
            ttl,
        )),
        delete_user: Arc::new(DeleteUserUseCase::new(repository, cache)),
    };

    Ok(Dependencies { state, readiness })
}

async fn build_cache(config: &Config) -> Result<Arc<dyn UserCache>> {
    let Some(redis_url) = config.redis_url.as_deref() else {
        tracing::info!("REDIS_URL is not configured; user cache is disabled");
        return Ok(Arc::new(NoOpUserCache));
    };

    let client = Client::open(redis_url).context("REDIS_URL is invalid")?;
    let connection = tokio::time::timeout(
        Duration::from_secs(config.redis_connect_timeout_seconds),
        client.get_connection_manager(),
    )
    .await;

    match connection {
        Ok(Ok(connection)) => Ok(Arc::new(RedisUserCache::new(connection))),
        Ok(Err(error)) => {
            tracing::warn!(error = ?error, "Redis is unavailable; user cache is disabled");
            Ok(Arc::new(NoOpUserCache))
        }
        Err(_) => {
            tracing::warn!(
                timeout_seconds = config.redis_connect_timeout_seconds,
                "Redis connection timed out; user cache is disabled"
            );
            Ok(Arc::new(NoOpUserCache))
        }
    }
}
