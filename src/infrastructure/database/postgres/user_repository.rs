use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    application::{
        job::NewJob,
        user::{RepositoryError, UserRegistrationRepository, UserRepository},
    },
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
    #[tracing::instrument(name = "infrastructure.postgres.user.create", skip(self, user), fields(db.system = "postgresql", user.id = %user.id()))]
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

    #[tracing::instrument(name = "infrastructure.postgres.user.find", skip(self), fields(db.system = "postgresql", user.id = %id))]
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

    #[tracing::instrument(name = "infrastructure.postgres.user.list", skip(self), fields(db.system = "postgresql", page.size = limit, page.offset = offset))]
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

    #[tracing::instrument(name = "infrastructure.postgres.user.update", skip(self, user), fields(db.system = "postgresql", user.id = %user.id()))]
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

    #[tracing::instrument(name = "infrastructure.postgres.user.delete", skip(self), fields(db.system = "postgresql", user.id = %id))]
    async fn delete(&self, id: UserId) -> Result<bool, RepositoryError> {
        let result = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id.as_uuid())
            .execute(&self.pool)
            .await
            .map_err(map_sqlx_error)?;
        Ok(result.rows_affected() == 1)
    }
}

#[async_trait]
impl UserRegistrationRepository for PostgresUserRepository {
    #[tracing::instrument(name = "infrastructure.postgres.user.create_with_job", skip(self, user, job), fields(db.system = "postgresql", user.id = %user.id(), job.id = %job.id, job.type = %job.job_type))]
    async fn create_with_job(&self, user: &User, job: &NewJob) -> Result<User, RepositoryError> {
        let max_attempts = i32::try_from(job.max_attempts).map_err(|error| {
            tracing::error!(%error, max_attempts = job.max_attempts, "max_attempts is too large");
            RepositoryError::Unavailable
        })?;
        if max_attempts == 0 {
            tracing::error!("max_attempts must be greater than zero");
            return Err(RepositoryError::Unavailable);
        }

        let mut transaction = self.pool.begin().await.map_err(map_sqlx_error)?;

        let created = sqlx::query_as::<_, UserRow>(
            r#"INSERT INTO users (id, email, display_name, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5)
               RETURNING id, email, display_name, created_at, updated_at"#,
        )
        .bind(user.id().as_uuid())
        .bind(user.email().as_str())
        .bind(user.display_name().as_str())
        .bind(user.created_at())
        .bind(user.updated_at())
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;

        sqlx::query(
            r#"INSERT INTO background_jobs (id, job_type, payload, trace_context, max_attempts)
               VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(job.id)
        .bind(&job.job_type)
        .bind(&job.payload)
        .bind(&job.trace_context)
        .bind(max_attempts)
        .execute(&mut *transaction)
        .await
        .map_err(map_sqlx_error)?;

        transaction.commit().await.map_err(map_sqlx_error)?;
        created.into_domain()
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
