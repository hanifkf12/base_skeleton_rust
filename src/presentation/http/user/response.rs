use serde::Serialize;

use crate::domain::user::User;

#[derive(Serialize)]
pub struct UserResponse {
    id: String,
    email: String,
    display_name: String,
    created_at: String,
    updated_at: String,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id().to_string(),
            email: user.email().as_str().to_owned(),
            display_name: user.display_name().as_str().to_owned(),
            created_at: user.created_at().to_rfc3339(),
            updated_at: user.updated_at().to_rfc3339(),
        }
    }
}

#[derive(Serialize)]
pub struct UserEnvelope {
    pub data: UserResponse,
}

#[derive(Serialize)]
pub struct UserListEnvelope {
    pub data: Vec<UserResponse>,
    pub page: u32,
    pub per_page: u32,
}
