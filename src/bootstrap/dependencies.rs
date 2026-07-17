use std::sync::Arc;

use anyhow::{Context, Result};
use redis::Client;
use sqlx::postgres::PgPoolOptions;

use crate::{
    application::user::{
        CreateUserUseCase, DeleteUserUseCase, GetUserUseCase, ListUsersUseCase, UpdateUserUseCase,
        UserCache, UserRepository,
    },
    config::Config,
    infrastructure::{cache::redis::RedisUserCache, database::postgres::PostgresUserRepository},
    presentation::http::AppState,
};

pub async fn build_state(config: &Config) -> Result<AppState> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .connect(&config.database_url)
        .await
        .context("could not connect to PostgreSQL")?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("could not run PostgreSQL migrations")?;

    let redis_client = Client::open(config.redis_url.as_str()).context("REDIS_URL is invalid")?;
    let redis_connection = redis_client
        .get_connection_manager()
        .await
        .context("could not connect to Redis")?;

    let repository: Arc<dyn UserRepository> = Arc::new(PostgresUserRepository::new(pool));
    let cache: Arc<dyn UserCache> = Arc::new(RedisUserCache::new(redis_connection));
    let ttl = config.user_cache_ttl_seconds;

    Ok(AppState {
        create_user: Arc::new(CreateUserUseCase::new(
            repository.clone(),
            cache.clone(),
            ttl,
        )),
        get_user: Arc::new(GetUserUseCase::new(repository.clone(), cache.clone(), ttl)),
        list_users: Arc::new(ListUsersUseCase::new(repository.clone())),
        update_user: Arc::new(UpdateUserUseCase::new(
            repository.clone(),
            cache.clone(),
            ttl,
        )),
        delete_user: Arc::new(DeleteUserUseCase::new(repository, cache)),
    })
}
