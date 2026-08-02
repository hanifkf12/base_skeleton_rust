mod database;
mod dependencies;
mod http;
mod shutdown;
mod worker;

use anyhow::{Result, ensure};

use crate::{
    cli::{Command, DatabaseCommand},
    config::{Config, OidcConfig, TelemetryConfig},
    telemetry,
};

pub async fn run(command: Command) -> Result<()> {
    dotenvy::dotenv().ok();
    let telemetry_config = TelemetryConfig::from_env()?;
    let telemetry = telemetry::init(&telemetry_config)?;
    let result = match command {
        Command::Http => run_http(&telemetry_config).await,
        Command::Worker => run_worker(&telemetry_config).await,
        Command::All { migrate } => run_all(migrate, &telemetry_config).await,
        Command::Db { command } => database::run(command).await,
        Command::MigrationCreate { name } => database::create_migration(&name),
    };
    telemetry.shutdown();
    result
}

async fn run_http(telemetry_config: &TelemetryConfig) -> Result<()> {
    let config = Config::from_env()?;
    let oidc_config = OidcConfig::from_env()?;
    let (sender, receiver) = shutdown::channel();
    let signal_task = tokio::spawn(shutdown::notify_on_signal(sender));
    let result = http::run(config, oidc_config, telemetry_config, receiver).await;
    signal_task.abort();
    result
}

async fn run_worker(_telemetry_config: &TelemetryConfig) -> Result<()> {
    let config = Config::from_env()?;
    let (sender, receiver) = shutdown::channel();
    let signal_task = tokio::spawn(shutdown::notify_on_signal(sender));
    let result = worker::run(config, receiver).await;
    signal_task.abort();
    result
}

async fn run_all(run_migrations: bool, telemetry_config: &TelemetryConfig) -> Result<()> {
    let config = Config::from_env()?;
    let (http_connections, worker_connections) =
        split_all_mode_connections(config.database_max_connections)?;
    if run_migrations {
        database::run(DatabaseCommand::Migrate).await?;
    }

    let oidc_config = OidcConfig::from_env()?;
    let (sender, receiver) = shutdown::channel();
    let signal_task = tokio::spawn(shutdown::notify_on_signal(sender.clone()));

    let mut http_config = config.clone();
    http_config.database_max_connections = http_connections;
    let mut worker_config = config.clone();
    worker_config.database_max_connections = worker_connections;

    let http = http::run(http_config, oidc_config, telemetry_config, receiver.clone());
    let worker = worker::run(worker_config, receiver);
    tokio::pin!(http, worker);

    let result = tokio::select! {
        http_result = &mut http => {
            let _ = sender.send(true);
            let worker_result = worker.await;
            http_result.and(worker_result)
        }
        worker_result = &mut worker => {
            let _ = sender.send(true);
            let http_result = http.await;
            worker_result.and(http_result)
        }
    };

    signal_task.abort();
    result
}

fn split_all_mode_connections(total: u32) -> Result<(u32, u32)> {
    ensure!(
        total >= 2,
        "all mode requires DATABASE_MAX_CONNECTIONS to be at least 2 so HTTP and worker each receive a connection"
    );
    let http = total / 2;
    Ok((http, total - http))
}

#[cfg(test)]
mod tests {
    use super::split_all_mode_connections;

    #[test]
    fn all_mode_requires_and_splits_connections() {
        assert!(split_all_mode_connections(1).is_err());
        assert_eq!(split_all_mode_connections(2).unwrap(), (1, 1));
        assert_eq!(split_all_mode_connections(5).unwrap(), (2, 3));
        assert_eq!(split_all_mode_connections(6).unwrap(), (3, 3));
    }
}
