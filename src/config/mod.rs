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
    pub job_poll_interval_milliseconds: u64,
    pub job_lease_timeout_seconds: u64,
    pub job_retry_base_seconds: u64,
    pub job_retry_max_seconds: u64,
    pub job_max_attempts: u32,
    pub job_worker_id: Option<String>,
}

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub audience: String,
    pub allowed_algorithms: Vec<String>,
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
        let job_poll_interval_milliseconds = parse_or("JOB_POLL_INTERVAL_MILLISECONDS", 1_000)?;
        let job_lease_timeout_seconds = parse_or("JOB_LEASE_TIMEOUT_SECONDS", 300)?;
        let job_retry_base_seconds = parse_or("JOB_RETRY_BASE_SECONDS", 5)?;
        let job_retry_max_seconds = parse_or("JOB_RETRY_MAX_SECONDS", 300)?;
        let job_max_attempts = parse_or("JOB_MAX_ATTEMPTS", 5)?;

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
        ensure!(
            job_poll_interval_milliseconds > 0,
            "JOB_POLL_INTERVAL_MILLISECONDS must be greater than zero"
        );
        ensure!(
            job_lease_timeout_seconds > 0,
            "JOB_LEASE_TIMEOUT_SECONDS must be greater than zero"
        );
        ensure!(
            job_retry_base_seconds > 0,
            "JOB_RETRY_BASE_SECONDS must be greater than zero"
        );
        ensure!(
            job_retry_max_seconds >= job_retry_base_seconds,
            "JOB_RETRY_MAX_SECONDS must be greater than or equal to JOB_RETRY_BASE_SECONDS"
        );
        ensure!(
            job_max_attempts > 0,
            "JOB_MAX_ATTEMPTS must be greater than zero"
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
            job_poll_interval_milliseconds,
            job_lease_timeout_seconds,
            job_retry_base_seconds,
            job_retry_max_seconds,
            job_max_attempts,
            job_worker_id: optional("JOB_WORKER_ID")?,
        })
    }

    pub fn migration_database_url_from_env() -> Result<String> {
        optional("MIGRATION_DATABASE_URL")?.map_or_else(|| required("DATABASE_URL"), Ok)
    }
}

impl OidcConfig {
    pub fn from_env() -> Result<Self> {
        let issuer_url = required_non_empty("OIDC_ISSUER_URL")?;
        let audience = required_non_empty("OIDC_AUDIENCE")?;
        let algorithms = env::var("OIDC_ALLOWED_ALGORITHMS").unwrap_or_else(|_| "RS256".to_owned());
        let allowed_algorithms = algorithms
            .split(',')
            .map(str::trim)
            .filter(|algorithm| !algorithm.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();

        ensure!(
            !allowed_algorithms.is_empty(),
            "OIDC_ALLOWED_ALGORITHMS must contain at least one algorithm"
        );

        Ok(Self {
            issuer_url,
            audience,
            allowed_algorithms,
        })
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("required environment variable {name} is missing"))
}

fn required_non_empty(name: &str) -> Result<String> {
    let value = required(name)?;
    ensure!(!value.trim().is_empty(), "{name} must not be empty");
    Ok(value)
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
