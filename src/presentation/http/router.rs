use axum::{
    Json, Router,
    routing::{get, post},
};
use serde_json::{Value, json};
use tower_http::trace::TraceLayer;

use super::{
    AppState,
    user::{create_user, delete_user, get_user, list_users, update_user},
};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/users", post(create_user).get(list_users))
        .route(
            "/api/v1/users/{id}",
            get(get_user).put(update_user).delete(delete_user),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
