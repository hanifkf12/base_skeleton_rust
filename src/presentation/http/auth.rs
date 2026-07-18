use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};

use crate::application::auth::{AccessTokenVerificationError, AccessTokenVerifier};

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
    use axum::body::Body;

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
}
