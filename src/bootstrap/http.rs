use anyhow::{Context, Result};
use tokio::{net::TcpListener, sync::watch};

use crate::{config::Config, presentation::http::build_router};

use super::{dependencies::build_dependencies, shutdown};

pub async fn run(config: Config, mut shutdown_receiver: watch::Receiver<bool>) -> Result<()> {
    let dependencies = build_dependencies(&config).await?;
    let router = build_router(
        dependencies.state,
        dependencies.readiness,
        config.request_timeout_seconds,
        config.max_request_body_bytes,
    );
    let listener = TcpListener::bind(config.server_address)
        .await
        .with_context(|| format!("could not bind server to {}", config.server_address))?;

    tracing::info!(address = %config.server_address, "HTTP server started");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown::wait(&mut shutdown_receiver).await;
        })
        .await
        .context("HTTP server failed")?;
    tracing::info!("HTTP server stopped");
    Ok(())
}
