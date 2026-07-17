use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use base_skeleton_rust::{
    application::{
        health::ReadinessCheck,
        job::NewJob,
        user::{
            CacheError, CreateUserUseCase, DeleteUserUseCase, GetUserUseCase, ListUsersUseCase,
            RepositoryError, UpdateUserUseCase, UserCache, UserRegistrationRepository,
            UserRepository,
        },
    },
    domain::user::{User, UserId},
    presentation::http::{AppState, build_router},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

#[derive(Default)]
struct InMemoryUserRepository {
    users: Mutex<HashMap<UserId, User>>,
}

#[async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn create(&self, user: &User) -> Result<User, RepositoryError> {
        let mut users = self.users.lock().unwrap();
        if users
            .values()
            .any(|existing| existing.email() == user.email())
        {
            return Err(RepositoryError::DuplicateEmail);
        }
        users.insert(user.id(), user.clone());
        Ok(user.clone())
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        Ok(self.users.lock().unwrap().get(&id).cloned())
    }

    async fn list(&self, limit: u32, offset: u64) -> Result<Vec<User>, RepositoryError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .values()
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn update(&self, user: &User) -> Result<Option<User>, RepositoryError> {
        let mut users = self.users.lock().unwrap();
        if !users.contains_key(&user.id()) {
            return Ok(None);
        }
        users.insert(user.id(), user.clone());
        Ok(Some(user.clone()))
    }

    async fn delete(&self, id: UserId) -> Result<bool, RepositoryError> {
        Ok(self.users.lock().unwrap().remove(&id).is_some())
    }
}

#[async_trait]
impl UserRegistrationRepository for InMemoryUserRepository {
    async fn create_with_job(&self, user: &User, _job: &NewJob) -> Result<User, RepositoryError> {
        self.create(user).await
    }
}

struct NoOpCache;

#[async_trait]
impl UserCache for NoOpCache {
    async fn get(&self, _id: UserId) -> Result<Option<User>, CacheError> {
        Ok(None)
    }

    async fn set(&self, _user: &User, _ttl_seconds: u64) -> Result<(), CacheError> {
        Ok(())
    }

    async fn delete(&self, _id: UserId) -> Result<(), CacheError> {
        Ok(())
    }
}

struct AlwaysReady;

#[async_trait]
impl ReadinessCheck for AlwaysReady {
    async fn is_ready(&self) -> bool {
        true
    }
}

fn app() -> axum::Router {
    let in_memory_repository = Arc::new(InMemoryUserRepository::default());
    let registration_repository: Arc<dyn UserRegistrationRepository> = in_memory_repository.clone();
    let repository: Arc<dyn UserRepository> = in_memory_repository;
    let cache: Arc<dyn UserCache> = Arc::new(NoOpCache);
    let state = AppState {
        create_user: Arc::new(CreateUserUseCase::new(
            registration_repository,
            cache.clone(),
            60,
            5,
        )),
        get_user: Arc::new(GetUserUseCase::new(repository.clone(), cache.clone(), 60)),
        list_users: Arc::new(ListUsersUseCase::new(repository.clone())),
        update_user: Arc::new(UpdateUserUseCase::new(
            repository.clone(),
            cache.clone(),
            60,
        )),
        delete_user: Arc::new(DeleteUserUseCase::new(repository, cache)),
    };

    build_router(state, Arc::new(AlwaysReady), 10, 65_536)
}

#[tokio::test]
async fn creates_and_reads_a_user_through_http() {
    let app = app();
    let create_response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/users",
            json!({"email": "ada@example.com", "display_name": "Ada Lovelace"}),
        ))
        .await
        .unwrap();

    assert_eq!(create_response.status(), StatusCode::CREATED);
    assert!(create_response.headers().contains_key("x-request-id"));
    let created = response_json(create_response).await;
    let id = created["data"]["id"].as_str().unwrap();

    let get_response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/users/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);
    let found = response_json(get_response).await;
    assert_eq!(found["data"]["email"], "ada@example.com");
}

#[tokio::test]
async fn returns_the_standard_error_envelope_for_invalid_json() {
    let response = app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/users")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_json");
}

#[tokio::test]
async fn exposes_liveness_and_readiness() {
    for path in ["/health/live", "/health/ready"] {
        let response = app()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

fn json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
