use axum::{
    Json,
    extract::rejection::{JsonRejection, QueryRejection},
    http::{HeaderValue, StatusCode, header},
    response::IntoResponse,
};
use serde::Serialize;

use crate::application::user::ApplicationError;

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    www_authenticate: Option<HeaderValue>,
}

impl ApiError {
    pub fn invalid_id() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_user_id",
            message: "user id must be a valid UUID".to_owned(),
            www_authenticate: None,
        }
    }

    pub fn unauthorized() -> Self {
        Self::authentication_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a Bearer access token is required",
            "Bearer",
        )
    }

    pub fn invalid_token() -> Self {
        Self::authentication_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "the Bearer access token is invalid",
            "Bearer error=\"invalid_token\"",
        )
    }

    pub fn insufficient_scope(scope: &'static str) -> Self {
        Self::authentication_error(
            StatusCode::FORBIDDEN,
            "insufficient_scope",
            "the access token does not grant the required scope",
            &format!("Bearer error=\"insufficient_scope\", scope=\"{scope}\""),
        )
    }

    pub fn authentication_unavailable() -> Self {
        Self::authentication_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "authentication_unavailable",
            "access token verification is temporarily unavailable",
            "Bearer",
        )
    }

    fn authentication_error(
        status: StatusCode,
        code: &'static str,
        message: &str,
        challenge: &str,
    ) -> Self {
        Self {
            status,
            code,
            message: message.to_owned(),
            www_authenticate: Some(
                HeaderValue::from_str(challenge).expect("authentication challenge is valid"),
            ),
        }
    }

    fn invalid_request(code: &'static str, message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message,
            www_authenticate: None,
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(error: JsonRejection) -> Self {
        Self::invalid_request("invalid_json", error.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(error: QueryRejection) -> Self {
        Self::invalid_request("invalid_query", error.body_text())
    }
}

impl From<ApplicationError> for ApiError {
    fn from(error: ApplicationError) -> Self {
        match error {
            ApplicationError::InvalidInput(error) => Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "validation_failed",
                message: error.to_string(),
                www_authenticate: None,
            },
            ApplicationError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                code: "user_not_found",
                message: error.to_string(),
                www_authenticate: None,
            },
            ApplicationError::EmailAlreadyExists => Self {
                status: StatusCode::CONFLICT,
                code: "email_already_exists",
                message: error.to_string(),
                www_authenticate: None,
            },
            ApplicationError::Conflict => Self {
                status: StatusCode::CONFLICT,
                code: "conflict",
                message: error.to_string(),
                www_authenticate: None,
            },
            ApplicationError::DependencyUnavailable => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "service_unavailable",
                message: error.to_string(),
                www_authenticate: None,
            },
        }
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        if self.status.is_server_error() {
            tracing::error!(
                http.response.status_code = self.status.as_u16(),
                error.code = self.code,
                "request failed"
            );
        } else {
            tracing::warn!(
                http.response.status_code = self.status.as_u16(),
                error.code = self.code,
                "request rejected"
            );
        }

        let mut response = (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response();
        if let Some(challenge) = self.www_authenticate {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, challenge);
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_not_found_to_404() {
        let error = ApiError::from(ApplicationError::NotFound);
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.code, "user_not_found");
    }
}
