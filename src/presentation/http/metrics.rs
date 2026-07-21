use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use subtle::ConstantTimeEq;

#[derive(Clone)]
struct MetricsState {
    token: String,
}

pub fn router(token: Option<String>) -> Router {
    match token {
        Some(token) => Router::new()
            .route("/metrics", get(metrics))
            .with_state(MetricsState { token }),
        None => Router::new(),
    }
}

async fn metrics(State(state): State<MetricsState>, headers: HeaderMap) -> Response {
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|candidate| candidate.as_bytes().ct_eq(state.token.as_bytes()).into());
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    match crate::telemetry::prometheus_text() {
        Some(Ok(metrics)) => (
            [(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            metrics,
        )
            .into_response(),
        _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
