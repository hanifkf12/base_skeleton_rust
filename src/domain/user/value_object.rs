use std::fmt::{Display, Formatter};

use uuid::Uuid;

use super::UserError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(Uuid);

impl UserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for UserId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl TryFrom<&str> for UserId {
    type Error = uuid::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(value).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email(String);

impl Email {
    pub fn parse(value: impl Into<String>) -> Result<Self, UserError> {
        let normalized = value.into().trim().to_lowercase();
        let (local, domain) = normalized.split_once('@').ok_or(UserError::InvalidEmail)?;

        if local.is_empty()
            || domain.is_empty()
            || !domain.contains('.')
            || normalized.len() > 254
            || normalized.chars().any(char::is_whitespace)
        {
            return Err(UserError::InvalidEmail);
        }

        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayName(String);

impl DisplayName {
    pub fn parse(value: impl Into<String>) -> Result<Self, UserError> {
        let value = value.into().trim().to_owned();
        let length = value.chars().count();
        if !(2..=100).contains(&length) {
            return Err(UserError::InvalidDisplayName);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_is_normalized() {
        let email = Email::parse("  Ada@Example.COM ").unwrap();
        assert_eq!(email.as_str(), "ada@example.com");
    }

    #[test]
    fn rejects_invalid_value_objects() {
        assert_eq!(Email::parse("not-an-email"), Err(UserError::InvalidEmail));
        assert_eq!(DisplayName::parse("x"), Err(UserError::InvalidDisplayName));
    }
}
