use axum::{
    Json,
    extract::{
        Path, Query, State,
        rejection::{JsonRejection, QueryRejection},
    },
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

#[tracing::instrument(name = "presentation.http.user.create", skip(state, request))]
pub async fn create_user(
    State(state): State<AppState>,
    request: Result<Json<CreateUserRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<UserEnvelope>), ApiError> {
    let Json(request) = request.map_err(ApiError::from)?;
    let user = state.create_user.execute(request.into()).await?;
    Ok((
        StatusCode::CREATED,
        Json(UserEnvelope { data: user.into() }),
    ))
}

#[tracing::instrument(name = "presentation.http.user.get", skip(state), fields(user.id = %id))]
pub async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<UserEnvelope>, ApiError> {
    let user = state.get_user.execute(parse_id(&id)?).await?;
    Ok(Json(UserEnvelope { data: user.into() }))
}

#[tracing::instrument(name = "presentation.http.user.list", skip(state, query))]
pub async fn list_users(
    State(state): State<AppState>,
    query: Result<Query<ListUsersQuery>, QueryRejection>,
) -> Result<Json<UserListEnvelope>, ApiError> {
    let Query(query) = query.map_err(ApiError::from)?;
    let input = ListUsersInput::from(query);
    let users = state.list_users.execute(input).await?;
    Ok(Json(UserListEnvelope {
        data: users.into_iter().map(UserResponse::from).collect(),
        page: input.page,
        per_page: input.per_page,
    }))
}

#[tracing::instrument(name = "presentation.http.user.update", skip(state, request), fields(user.id = %id))]
pub async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Result<Json<UpdateUserRequest>, JsonRejection>,
) -> Result<Json<UserEnvelope>, ApiError> {
    let Json(request) = request.map_err(ApiError::from)?;
    let user = state
        .update_user
        .execute(parse_id(&id)?, request.into())
        .await?;
    Ok(Json(UserEnvelope { data: user.into() }))
}

#[tracing::instrument(name = "presentation.http.user.delete", skip(state), fields(user.id = %id))]
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
