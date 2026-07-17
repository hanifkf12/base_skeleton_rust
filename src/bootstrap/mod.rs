mod database;
mod dependencies;
mod http;
mod shutdown;
mod worker;

use anyhow::Result;

use crate::{
    cli::{Command, DatabaseCommand},
    config::Config,
};

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Http => run_http().await,
        Command::Worker => run_worker().await,
        Command::All { migrate } => run_all(migrate).await,
        Command::Db { command } => database::run(command).await,
    }
}

async fn run_http() -> Result<()> {
    let config = Config::from_env()?;
    let (sender, receiver) = shutdown::channel();
    let signal_task = tokio::spawn(shutdown::notify_on_signal(sender));
    let result = http::run(config, receiver).await;
    signal_task.abort();
    result
}

async fn run_worker() -> Result<()> {
    let config = Config::from_env()?;
    let (sender, receiver) = shutdown::channel();
    let signal_task = tokio::spawn(shutdown::notify_on_signal(sender));
    let result = worker::run(config, receiver).await;
    signal_task.abort();
    result
}

async fn run_all(run_migrations: bool) -> Result<()> {
    if run_migrations {
        database::run(DatabaseCommand::Migrate).await?;
    }

    let config = Config::from_env()?;
    let (sender, receiver) = shutdown::channel();
    let signal_task = tokio::spawn(shutdown::notify_on_signal(sender.clone()));
    let http = http::run(config.clone(), receiver.clone());
    let worker = worker::run(config, receiver);
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
