use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UserError {
    #[error("email address is invalid")]
    InvalidEmail,
    #[error("display name must contain between 2 and 100 characters")]
    InvalidDisplayName,
}
