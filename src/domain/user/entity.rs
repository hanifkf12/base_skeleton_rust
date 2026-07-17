use chrono::{DateTime, Utc};

use super::{DisplayName, Email, UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    id: UserId,
    email: Email,
    display_name: DisplayName,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl User {
    pub fn new(email: Email, display_name: DisplayName) -> Self {
        let now = Utc::now();
        Self {
            id: UserId::new(),
            email,
            display_name,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn restore(
        id: UserId,
        email: Email,
        display_name: DisplayName,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            email,
            display_name,
            created_at,
            updated_at,
        }
    }

    pub fn update_profile(&mut self, email: Email, display_name: DisplayName) {
        self.email = email;
        self.display_name = display_name;
        self.updated_at = Utc::now();
    }

    pub const fn id(&self) -> UserId {
        self.id
    }
    pub const fn email(&self) -> &Email {
        &self.email
    }
    pub const fn display_name(&self) -> &DisplayName {
        &self.display_name
    }
    pub const fn created_at(&self) -> &DateTime<Utc> {
        &self.created_at
    }
    pub const fn updated_at(&self) -> &DateTime<Utc> {
        &self.updated_at
    }
}
