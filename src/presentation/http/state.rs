use std::sync::Arc;

use crate::application::user::{
    CreateUserUseCase, DeleteUserUseCase, GetUserUseCase, ListUsersUseCase, UpdateUserUseCase,
};

#[derive(Clone)]
pub struct AppState {
    pub create_user: Arc<CreateUserUseCase>,
    pub get_user: Arc<GetUserUseCase>,
    pub list_users: Arc<ListUsersUseCase>,
    pub update_user: Arc<UpdateUserUseCase>,
    pub delete_user: Arc<DeleteUserUseCase>,
}
