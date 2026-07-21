use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::{net::TcpListener, sync::watch};

use crate::{
    application::auth::AccessTokenVerifier,
    config::{Config, OidcConfig},
    infrastructure::oidc::OidcAccessTokenVerifier,
    presentation::http::{RouterConfig, build_router},
};

use super::{dependencies::build_dependencies, shutdown};

pub async fn run(
    config: Config,
    oidc_config: OidcConfig,
    mut shutdown_receiver: watch::Receiver<bool>,
) -> Result<()> {
    let access_token_verifier: Arc<dyn AccessTokenVerifier> =
        OidcAccessTokenVerifier::discover(&oidc_config).await?;
    let dependencies = build_dependencies(&config).await?;
    let router = build_router(
        dependencies.state,
        dependencies.readiness,
        access_token_verifier,
        RouterConfig {
            request_timeout_seconds: config.request_timeout_seconds,
            max_request_body_bytes: config.max_request_body_bytes,
            rate_limit_requests_per_minute: config.rate_limit_requests_per_minute,
            rate_limit_burst: config.rate_limit_burst,
            trusted_proxy_cidrs: config.trusted_proxy_cidrs,
            metrics_prometheus_bearer_token: config.metrics_prometheus_bearer_token,
        },
    );
    let listener = TcpListener::bind(config.server_address)
        .await
        .with_context(|| format!("could not bind server to {}", config.server_address))?;

    tracing::info!(address = %config.server_address, "HTTP server started");
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown::wait(&mut shutdown_receiver).await;
    })
    .await
    .context("HTTP server failed")?;
    tracing::info!("HTTP server stopped");
    Ok(())
}
