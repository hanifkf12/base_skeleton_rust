use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{header, request::Parts},
    middleware::Next,
    response::Response,
};

use crate::application::auth::{
    AccessTokenVerificationError, AccessTokenVerifier, AuthenticatedPrincipal,
};

use super::error::ApiError;

#[derive(Clone)]
pub struct ScopeRequirement {
    verifier: Arc<dyn AccessTokenVerifier>,
    scope: &'static str,
}

impl ScopeRequirement {
    pub fn new(verifier: Arc<dyn AccessTokenVerifier>, scope: &'static str) -> Self {
        Self { verifier, scope }
    }
}

impl<S> FromRequestParts<S> for AuthenticatedPrincipal
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<AuthenticatedPrincipal>()
            .cloned()
            .ok_or_else(ApiError::unauthorized)
    }
}

pub async fn require_scope(
    State(requirement): State<ScopeRequirement>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let access_token = bearer_token(&request).ok_or_else(ApiError::unauthorized)?;
    let principal =
        requirement
            .verifier
            .verify(access_token)
            .await
            .map_err(|error| match error {
                AccessTokenVerificationError::InvalidToken => ApiError::invalid_token(),
                AccessTokenVerificationError::AuthenticationUnavailable => {
                    ApiError::authentication_unavailable()
                }
            })?;

    if !principal.has_scope(requirement.scope) {
        return Err(ApiError::insufficient_scope(requirement.scope));
    }

    request.extensions_mut().insert(principal);
    Ok(next.run(request).await)
}

fn bearer_token(request: &Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer")
        || token.is_empty()
        || token.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(token)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use axum::{body::Body, http::StatusCode, response::IntoResponse};

    use super::*;

    #[test]
    fn accepts_only_one_non_empty_bearer_credential() {
        let request = Request::builder()
            .header(header::AUTHORIZATION, "bearer token-value")
            .body(Body::empty())
            .unwrap();
        assert_eq!(bearer_token(&request), Some("token-value"));

        for value in ["Basic abc", "Bearer", "Bearer ", "Bearer one two"] {
            let request = Request::builder()
                .header(header::AUTHORIZATION, value)
                .body(Body::empty())
                .unwrap();
            assert_eq!(bearer_token(&request), None);
        }
    }

    #[tokio::test]
    async fn principal_extractor_reads_verified_principal_from_extensions() {
        let principal = AuthenticatedPrincipal::new(
            "operator-123".to_owned(),
            HashSet::from(["users:read".to_owned()]),
        );
        let mut request = Request::builder().body(Body::empty()).unwrap();
        request.extensions_mut().insert(principal.clone());
        let (mut parts, _) = request.into_parts();

        let extracted = AuthenticatedPrincipal::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(extracted, principal);
    }

    #[tokio::test]
    async fn principal_extractor_rejects_missing_principal() {
        let request = Request::builder().body(Body::empty()).unwrap();
        let (mut parts, _) = request.into_parts();

        let error = AuthenticatedPrincipal::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
