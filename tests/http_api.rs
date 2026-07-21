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
        auth::{AccessTokenVerificationError, AccessTokenVerifier, AuthenticatedPrincipal},
        health::ReadinessCheck,
        job::NewJob,
        user::{
            CacheError, CreateUserUseCase, DeleteUserUseCase, GetUserUseCase, ListUsersUseCase,
            RepositoryError, TraceContextProvider, UpdateUserUseCase, UserCache,
            UserRegistrationRepository, UserRepository,
        },
    },
    domain::user::{User, UserId},
    presentation::http::{AppState, RouterConfig, build_router},
};
use chrono::{DateTime, Utc};
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

    async fn update(
        &self,
        user: &User,
        expected_updated_at: &DateTime<Utc>,
    ) -> Result<Option<User>, RepositoryError> {
        let mut users = self.users.lock().unwrap();
        match users.get(&user.id()) {
            Some(existing) if existing.updated_at() == expected_updated_at => {
                users.insert(user.id(), user.clone());
                Ok(Some(user.clone()))
            }
            Some(_) => Ok(None),
            None => Ok(None),
        }
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

struct NoOpTraceContext;

impl TraceContextProvider for NoOpTraceContext {
    fn current(&self) -> Value {
        json!({})
    }
}

fn app() -> axum::Router {
    app_with_verifier(Arc::new(FakeAccessTokenVerifier::with_scopes(&[
        "users:read",
        "users:write",
    ])))
}

struct FakeAccessTokenVerifier {
    result: Result<AuthenticatedPrincipal, AccessTokenVerificationError>,
}

impl FakeAccessTokenVerifier {
    fn with_scopes(scopes: &[&str]) -> Self {
        Self {
            result: Ok(AuthenticatedPrincipal::new(
                "test-subject".to_owned(),
                scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            )),
        }
    }

    fn unavailable() -> Self {
        Self {
            result: Err(AccessTokenVerificationError::AuthenticationUnavailable),
        }
    }
}

#[async_trait]
impl AccessTokenVerifier for FakeAccessTokenVerifier {
    async fn verify(
        &self,
        access_token: &str,
    ) -> Result<AuthenticatedPrincipal, AccessTokenVerificationError> {
        if access_token != "valid-token" {
            return Err(AccessTokenVerificationError::InvalidToken);
        }
        self.result.clone()
    }
}

fn app_with_verifier(verifier: Arc<dyn AccessTokenVerifier>) -> axum::Router {
    app_with_config(verifier, 120, 30, None)
}

fn app_with_config(
    verifier: Arc<dyn AccessTokenVerifier>,
    requests_per_minute: u32,
    burst: u32,
    metrics_token: Option<String>,
) -> axum::Router {
    let in_memory_repository = Arc::new(InMemoryUserRepository::default());
    let registration_repository: Arc<dyn UserRegistrationRepository> = in_memory_repository.clone();
    let repository: Arc<dyn UserRepository> = in_memory_repository;
    let cache: Arc<dyn UserCache> = Arc::new(NoOpCache);
    let trace_context: Arc<dyn TraceContextProvider> = Arc::new(NoOpTraceContext);
    let state = AppState {
        create_user: Arc::new(CreateUserUseCase::new(
            registration_repository,
            cache.clone(),
            trace_context,
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

    build_router(
        state,
        Arc::new(AlwaysReady),
        verifier,
        RouterConfig {
            request_timeout_seconds: 10,
            max_request_body_bytes: 65_536,
            rate_limit_requests_per_minute: requests_per_minute,
            rate_limit_burst: burst,
            trusted_proxy_cidrs: Vec::new(),
            metrics_prometheus_bearer_token: metrics_token,
        },
    )
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
                .header(header::AUTHORIZATION, "Bearer valid-token")
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
                .header(header::AUTHORIZATION, "Bearer valid-token")
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

#[tokio::test]
async fn rate_limits_only_api_routes_with_retry_after_and_json_error() {
    let verifier = Arc::new(FakeAccessTokenVerifier::with_scopes(&["users:read"]));
    let app = app_with_config(verifier, 1, 2, None);
    for _ in 0..10 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = app
        .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(response.headers().contains_key(header::RETRY_AFTER));
    assert_eq!(
        response_json(response).await["error"]["code"],
        "rate_limit_exceeded"
    );
}

#[tokio::test]
async fn rate_limit_bucket_refills() {
    let verifier = Arc::new(FakeAccessTokenVerifier::with_scopes(&["users:read"]));
    let app = app_with_config(verifier, 600, 1, None);
    let first = app
        .clone()
        .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let limited = app
        .clone()
        .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    let refilled = app
        .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
        .await
        .unwrap();
    assert_eq!(refilled.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_route_is_disabled_or_requires_its_bearer_token() {
    let disabled = app();
    let response = disabled
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let verifier = Arc::new(FakeAccessTokenVerifier::with_scopes(&[]));
    let enabled = app_with_config(verifier, 120, 30, Some("scrape-secret".to_owned()));
    for authorization in [None, Some("Bearer wrong-secret")] {
        let mut request = Request::builder().uri("/metrics");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = enabled
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = enabled
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header(header::AUTHORIZATION, "Bearer scrape-secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // The binary installs the exporter before building the router. This
    // integration router deliberately does not initialize global telemetry.
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn rejects_missing_malformed_and_invalid_bearer_tokens() {
    for authorization in [None, Some("Basic abc"), Some("Bearer invalid-token")] {
        let mut request = Request::builder().uri("/api/v1/users");
        if let Some(value) = authorization {
            request = request.header(header::AUTHORIZATION, value);
        }
        let response = app()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Bearer")
        );
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unauthorized");
    }
}

#[tokio::test]
async fn enforces_read_and_write_scopes_for_every_user_method() {
    let read_only = Arc::new(FakeAccessTokenVerifier::with_scopes(&["users:read"]));
    for method in ["GET", "HEAD"] {
        let response = app_with_verifier(read_only.clone())
            .oneshot(authorized_request(method, "/api/v1/users", Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for (method, path) in [
        ("POST", "/api/v1/users"),
        ("PUT", "/api/v1/users/00000000-0000-0000-0000-000000000000"),
        (
            "DELETE",
            "/api/v1/users/00000000-0000-0000-0000-000000000000",
        ),
    ] {
        let response = app_with_verifier(read_only.clone())
            .oneshot(authorized_request(method, path, Body::empty()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer error=\"insufficient_scope\", scope=\"users:write\""
        );
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "insufficient_scope");
    }

    let write_only = Arc::new(FakeAccessTokenVerifier::with_scopes(&["users:write"]));
    let response = app_with_verifier(write_only.clone())
        .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app_with_verifier(write_only)
        .oneshot(json_request(
            "POST",
            "/api/v1/users",
            json!({"email": "grace@example.com", "display_name": "Grace Hopper"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn maps_provider_refresh_failure_to_authentication_unavailable() {
    let response = app_with_verifier(Arc::new(FakeAccessTokenVerifier::unavailable()))
        .oneshot(authorized_request("GET", "/api/v1/users", Body::empty()))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
        "Bearer"
    );
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "authentication_unavailable");
}

fn json_request(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer valid-token")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn authorized_request(method: &str, uri: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer valid-token")
        .body(body)
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
