use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;

use crate::application::health::ReadinessCheck;

#[derive(Clone)]
struct HealthState {
    readiness: Arc<dyn ReadinessCheck>,
}

pub fn router(readiness: Arc<dyn ReadinessCheck>) -> Router {
    Router::new()
        .route("/health", get(live))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(HealthState { readiness })
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[tracing::instrument(name = "presentation.http.health.live")]
async fn live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[tracing::instrument(name = "presentation.http.health.ready", skip(state))]
async fn ready(State(state): State<HealthState>) -> (StatusCode, Json<HealthResponse>) {
    if state.readiness.is_ready().await {
        (StatusCode::OK, Json(HealthResponse { status: "ready" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "not_ready",
            }),
        )
    }
}
