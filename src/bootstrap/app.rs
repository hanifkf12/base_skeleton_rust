use anyhow::{Context, Result};
use tokio::net::TcpListener;

use crate::{config::Config, presentation::http::build_router, telemetry};

use super::dependencies::build_dependencies;

pub async fn run() -> Result<()> {
    dotenvy::dotenv().ok();
    telemetry::init();

    let config = Config::from_env()?;
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
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")
}

pub(super) async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
