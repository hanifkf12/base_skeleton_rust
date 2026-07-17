use std::sync::Arc;

use crate::domain::user::User;

use super::super::{ApplicationError, ListUsersInput, UserRepository};

pub struct ListUsersUseCase {
    repository: Arc<dyn UserRepository>,
}

impl ListUsersUseCase {
    pub fn new(repository: Arc<dyn UserRepository>) -> Self {
        Self { repository }
    }

    #[tracing::instrument(
        name = "application.user.list",
        skip(self),
        fields(page = input.page, per_page = input.per_page)
    )]
    pub async fn execute(&self, input: ListUsersInput) -> Result<Vec<User>, ApplicationError> {
        let input = input.normalized();
        self.repository
            .list(input.per_page, input.offset())
            .await
            .map_err(Into::into)
    }
}
