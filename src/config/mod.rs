use std::{env, net::SocketAddr};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub server_address: SocketAddr,
    pub database_url: String,
    pub database_max_connections: u32,
    pub redis_url: String,
    pub user_cache_ttl_seconds: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let host = env::var("APP_HOST").unwrap_or_else(|_| "0.0.0.0".to_owned());
        let port = env::var("APP_PORT").unwrap_or_else(|_| "3000".to_owned());
        let server_address = format!("{host}:{port}")
            .parse()
            .context("APP_HOST and APP_PORT must form a valid socket address")?;

        Ok(Self {
            server_address,
            database_url: required("DATABASE_URL")?,
            database_max_connections: parse_or("DATABASE_MAX_CONNECTIONS", 10)?,
            redis_url: required("REDIS_URL")?,
            user_cache_ttl_seconds: parse_or("USER_CACHE_TTL_SECONDS", 300)?,
        })
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

fn parse_or<T>(name: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .with_context(|| format!("{name} has an invalid value")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("could not read {name}")),
    }
}
