use std::collections::HashSet;

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedPrincipal {
    subject: String,
    scopes: HashSet<String>,
}

impl AuthenticatedPrincipal {
    pub fn new(subject: String, scopes: HashSet<String>) -> Self {
        Self { subject, scopes }
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains(scope)
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AccessTokenVerificationError {
    #[error("access token is invalid")]
    InvalidToken,
    #[error("the authentication provider is unavailable")]
    AuthenticationUnavailable,
}

#[async_trait]
pub trait AccessTokenVerifier: Send + Sync {
    async fn verify(
        &self,
        access_token: &str,
    ) -> Result<AuthenticatedPrincipal, AccessTokenVerificationError>;
}
