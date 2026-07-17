use std::{env, net::SocketAddr};

use anyhow::{Context, Result, ensure};

#[derive(Clone)]
pub struct Config {
    pub server_address: SocketAddr,
    pub database_url: String,
    pub database_max_connections: u32,
    pub redis_url: Option<String>,
    pub redis_connect_timeout_seconds: u64,
    pub user_cache_ttl_seconds: u64,
    pub request_timeout_seconds: u64,
    pub max_request_body_bytes: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let host = env::var("APP_HOST").unwrap_or_else(|_| "0.0.0.0".to_owned());
        let port = env::var("APP_PORT").unwrap_or_else(|_| "3000".to_owned());
        let server_address = format!("{host}:{port}")
            .parse()
            .context("APP_HOST and APP_PORT must form a valid socket address")?;
        let database_max_connections = parse_or("DATABASE_MAX_CONNECTIONS", 10)?;
        let redis_connect_timeout_seconds = parse_or("REDIS_CONNECT_TIMEOUT_SECONDS", 3)?;
        let user_cache_ttl_seconds = parse_or("USER_CACHE_TTL_SECONDS", 300)?;
        let request_timeout_seconds = parse_or("REQUEST_TIMEOUT_SECONDS", 10)?;
        let max_request_body_bytes = parse_or("MAX_REQUEST_BODY_BYTES", 65_536)?;

        ensure!(
            database_max_connections > 0,
            "DATABASE_MAX_CONNECTIONS must be greater than zero"
        );
        ensure!(
            redis_connect_timeout_seconds > 0,
            "REDIS_CONNECT_TIMEOUT_SECONDS must be greater than zero"
        );
        ensure!(
            user_cache_ttl_seconds > 0,
            "USER_CACHE_TTL_SECONDS must be greater than zero"
        );
        ensure!(
            request_timeout_seconds > 0,
            "REQUEST_TIMEOUT_SECONDS must be greater than zero"
        );
        ensure!(
            max_request_body_bytes > 0,
            "MAX_REQUEST_BODY_BYTES must be greater than zero"
        );

        Ok(Self {
            server_address,
            database_url: required("DATABASE_URL")?,
            database_max_connections,
            redis_url: optional("REDIS_URL")?,
            redis_connect_timeout_seconds,
            user_cache_ttl_seconds,
            request_timeout_seconds,
            max_request_body_bytes,
        })
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

fn optional(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {name}")),
    }
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
