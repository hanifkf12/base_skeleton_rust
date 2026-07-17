use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};

use crate::{
    application::user::ListUsersInput,
    domain::user::UserId,
    presentation::http::{AppState, error::ApiError},
};

use super::{
    request::{CreateUserRequest, ListUsersQuery, UpdateUserRequest},
    response::{UserEnvelope, UserListEnvelope, UserResponse},
};

pub async fn create_user(
    State(state): State<AppState>,
    Json(request): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserEnvelope>), ApiError> {
    let user = state.create_user.execute(request.into()).await?;
    Ok((
        StatusCode::CREATED,
        Json(UserEnvelope { data: user.into() }),
    ))
}

pub async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<UserEnvelope>, ApiError> {
    let user = state.get_user.execute(parse_id(&id)?).await?;
    Ok(Json(UserEnvelope { data: user.into() }))
}

pub async fn list_users(
    State(state): State<AppState>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<UserListEnvelope>, ApiError> {
    let input = ListUsersInput::from(query);
    let users = state.list_users.execute(input).await?;
    Ok(Json(UserListEnvelope {
        data: users.into_iter().map(UserResponse::from).collect(),
        page: input.page,
        per_page: input.per_page,
    }))
}

pub async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<UserEnvelope>, ApiError> {
    let user = state
        .update_user
        .execute(parse_id(&id)?, request.into())
        .await?;
    Ok(Json(UserEnvelope { data: user.into() }))
}

pub async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.delete_user.execute(parse_id(&id)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_id(value: &str) -> Result<UserId, ApiError> {
    UserId::try_from(value).map_err(|_| ApiError::invalid_id())
}
