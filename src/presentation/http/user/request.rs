use serde::Deserialize;

use crate::application::user::{CreateUserInput, ListUsersInput, UpdateUserInput};

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub display_name: String,
}

impl From<CreateUserRequest> for CreateUserInput {
    fn from(value: CreateUserRequest) -> Self {
        Self {
            email: value.email,
            display_name: value.display_name,
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    pub email: String,
    pub display_name: String,
}

impl From<UpdateUserRequest> for UpdateUserInput {
    fn from(value: UpdateUserRequest) -> Self {
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
