use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    application::user::{RepositoryError, UserRepository},
    domain::user::{DisplayName, Email, User, UserId},
};

pub struct PostgresUserRepository {
    pool: PgPool,
}

impl PostgresUserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    display_name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl UserRow {
    fn into_domain(self) -> Result<User, RepositoryError> {
        let email = Email::parse(self.email).map_err(|error| {
            tracing::error!(%error, "database contains an invalid user email");
            RepositoryError::Unavailable
        })?;
        let display_name = DisplayName::parse(self.display_name).map_err(|error| {
            tracing::error!(%error, "database contains an invalid display name");
            RepositoryError::Unavailable
        })?;
        Ok(User::restore(
            UserId::from_uuid(self.id),
            email,
            display_name,
            self.created_at,
            self.updated_at,
        ))
    }
}

#[async_trait]
impl UserRepository for PostgresUserRepository {
    async fn create(&self, user: &User) -> Result<User, RepositoryError> {
        sqlx::query_as::<_, UserRow>(
            r#"INSERT INTO users (id, email, display_name, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id, email, display_name, created_at, updated_at"#,
        )
        .bind(user.id().as_uuid())
        .bind(user.email().as_str())
        .bind(user.display_name().as_str())
        .bind(user.created_at())
        .bind(user.updated_at())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .into_domain()
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, RepositoryError> {
        sqlx::query_as::<_, UserRow>(
            "SELECT id, email, display_name, created_at, updated_at FROM users WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(UserRow::into_domain)
        .transpose()
    }

    async fn list(&self, limit: u32, offset: u64) -> Result<Vec<User>, RepositoryError> {
        let rows = sqlx::query_as::<_, UserRow>(
            r#"SELECT id, email, display_name, created_at, updated_at
               FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2"#,
        )
        .bind(i64::from(limit))
        .bind(i64::try_from(offset).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        rows.into_iter().map(UserRow::into_domain).collect()
    }

    async fn update(&self, user: &User) -> Result<Option<User>, RepositoryError> {
        sqlx::query_as::<_, UserRow>(
            r#"UPDATE users SET email = $2, display_name = $3, updated_at = $4
               WHERE id = $1
               RETURNING id, email, display_name, created_at, updated_at"#,
        )
        .bind(user.id().as_uuid())
        .bind(user.email().as_str())
        .bind(user.display_name().as_str())
        .bind(user.updated_at())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_sqlx_error)?
        .map(UserRow::into_domain)
        .transpose()
    }

    async fn delete(&self, id: UserId) -> Result<bool, RepositoryError> {
        let result = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        Ok(result.rows_affected() == 1)
    }
}

fn map_sqlx_error(error: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(database_error) = &error
        && database_error.is_unique_violation()
        && database_error.constraint() == Some("users_email_unique")
    {
        return RepositoryError::DuplicateEmail;
    }

    tracing::error!(error = ?error, "PostgreSQL user operation failed");
    RepositoryError::Unavailable
}
