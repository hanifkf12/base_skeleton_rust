use thiserror::Error;

use crate::domain::user::UserError;

use super::RepositoryError;

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    InvalidInput(#[from] UserError),
    #[error("user was not found")]
    NotFound,
    #[error("a user with this email already exists")]
    EmailAlreadyExists,
    #[error("a required dependency is unavailable")]
    DependencyUnavailable,
}

impl From<RepositoryError> for ApplicationError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::DuplicateEmail => Self::EmailAlreadyExists,
            RepositoryError::Unavailable => Self::DependencyUnavailable,
        }
    }
}
