use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Body,
    extract::DefaultBodyLimit,
    http::{StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use ipnet::IpNet;
use tower::ServiceBuilder;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::application::{auth::AccessTokenVerifier, health::ReadinessCheck};

use super::{
    AppState,
    auth::{ScopeRequirement, require_scope},
    health, metrics,
    rate_limit::{self, TrustedProxyIpKeyExtractor},
    user::{create_user, delete_user, get_user, list_users, update_user},
};

#[derive(Clone)]
pub struct RouterConfig {
    pub request_timeout_seconds: u64,
    pub max_request_body_bytes: usize,
    pub rate_limit_requests_per_minute: u32,
    pub rate_limit_burst: u32,
    pub trusted_proxy_cidrs: Vec<IpNet>,
    pub metrics_prometheus_bearer_token: Option<String>,
}

pub fn build_router(
    state: AppState,
    readiness: Arc<dyn ReadinessCheck>,
    access_token_verifier: Arc<dyn AccessTokenVerifier>,
    config: RouterConfig,
) -> Router {
    let request_id_header = header::HeaderName::from_static("x-request-id");
    let read_api = Router::new()
        .route("/api/v1/users", get(list_users))
        .route("/api/v1/users/{id}", get(get_user))
        .route_layer(middleware::from_fn_with_state(
            ScopeRequirement::new(access_token_verifier.clone(), "users:read"),
            require_scope,
        ));
    let write_api = Router::new()
        .route("/api/v1/users", post(create_user))
        .route(
            "/api/v1/users/{id}",
            axum::routing::put(update_user).delete(delete_user),
        )
        .route_layer(middleware::from_fn_with_state(
            ScopeRequirement::new(access_token_verifier, "users:write"),
            require_scope,
        ));
    let refill_period =
        Duration::from_secs_f64(60.0 / f64::from(config.rate_limit_requests_per_minute));
    let mut governor_builder = GovernorConfigBuilder::default();
    governor_builder
        .period(refill_period)
        .burst_size(config.rate_limit_burst);
    let mut governor_builder = governor_builder.key_extractor(TrustedProxyIpKeyExtractor::new(
        config.trusted_proxy_cidrs.clone(),
    ));
    let governor = governor_builder
        .finish()
        .expect("validated rate limit configuration is non-zero");
    let api = Router::new()
        .merge(read_api)
        .merge(write_api)
        .route_layer(GovernorLayer::new(governor).error_handler(rate_limit::error_response))
        .with_state(state);

    Router::new()
        .merge(health::router(readiness))
        .merge(metrics::router(
            config.metrics_prometheus_bearer_token.clone(),
        ))
        .merge(api)
        .layer(DefaultBodyLimit::max(config.max_request_body_bytes))
        .layer(middleware::from_fn(record_http_metrics))
        .layer(
            ServiceBuilder::new()
                .layer(SetSensitiveRequestHeadersLayer::new([
                    header::AUTHORIZATION,
                    header::COOKIE,
                ]))
                .layer(SetRequestIdLayer::new(
                    request_id_header.clone(),
                    MakeRequestUuid,
                ))
                .layer(
                    TraceLayer::new_for_http()
                        .make_span_with(crate::telemetry::http_span)
                        .on_response(
                            |response: &axum::response::Response,
                             _latency: Duration,
                             span: &tracing::Span| {
                                let status = response.status();
                                span.record("http.response.status_code", status.as_u16());
                                if status.is_server_error() {
                                    span.record("otel.status_code", "ERROR");
                                    span.record("otel.status_description", status.to_string());
                                }
                            },
                        ),
                )
                .layer(PropagateRequestIdLayer::new(request_id_header))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    Duration::from_secs(config.request_timeout_seconds),
                )),
        )
}

async fn record_http_metrics(request: axum::http::Request<Body>, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let route = normalized_route(request.uri().path());
    crate::telemetry::http_request_started(&method, route);
    let started = Instant::now();
    let response = next.run(request).await;
    crate::telemetry::http_request_finished(
        &method,
        route,
        response.status().as_u16(),
        started.elapsed(),
    );
    response
}

fn normalized_route(path: &str) -> &'static str {
    match path {
        "/health" => "/health",
        "/health/live" => "/health/live",
        "/health/ready" => "/health/ready",
        "/metrics" => "/metrics",
        "/api/v1/users" => "/api/v1/users",
        _ if path.starts_with("/api/v1/users/") => "/api/v1/users/{id}",
        _ => "unmatched",
    }
}
