use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{StatusCode, header},
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveRequestHeadersLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::application::health::ReadinessCheck;

use super::{
    AppState, health,
    user::{create_user, delete_user, get_user, list_users, update_user},
};

pub fn build_router(
    state: AppState,
    readiness: Arc<dyn ReadinessCheck>,
    request_timeout_seconds: u64,
    max_request_body_bytes: usize,
) -> Router {
    let request_id_header = header::HeaderName::from_static("x-request-id");
    let api = Router::new()
        .route("/api/v1/users", post(create_user).get(list_users))
        .route(
            "/api/v1/users/{id}",
            get(get_user).put(update_user).delete(delete_user),
        )
        .with_state(state);

    Router::new()
        .merge(health::router(readiness))
        .merge(api)
        .layer(DefaultBodyLimit::max(max_request_body_bytes))
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
                    Duration::from_secs(request_timeout_seconds),
                )),
        )
}
