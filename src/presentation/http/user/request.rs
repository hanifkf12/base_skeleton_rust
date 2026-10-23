use serde::Deserialize;

use crate::application::user::{ListUsersInput, UserInput};

#[derive(Deserialize)]
pub struct UserRequest {
    pub email: String,
    pub display_name: String,
}

pub type CreateUserRequest = UserRequest;
pub type UpdateUserRequest = UserRequest;

impl From<UserRequest> for UserInput {
    fn from(value: UserRequest) -> Self {
        Self {
            email: value.email,
            display_name: value.display_name,
        }
    }
}

#[derive(Deserialize)]
pub struct ListUsersQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

impl From<ListUsersQuery> for ListUsersInput {
    fn from(value: ListUsersQuery) -> Self {
        Self {
            page: value.page,
            per_page: value.per_page,
        }
        .normalized()
    }
}

const fn default_page() -> u32 {
    1
}
const fn default_per_page() -> u32 {
    20
}
