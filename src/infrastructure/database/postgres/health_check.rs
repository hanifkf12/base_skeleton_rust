use async_trait::async_trait;
use sqlx::PgPool;

use crate::application::health::ReadinessCheck;

pub struct PostgresReadinessCheck {
    pool: PgPool,
}

impl PostgresReadinessCheck {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ReadinessCheck for PostgresReadinessCheck {
    #[tracing::instrument(name = "infrastructure.postgres.readiness", skip(self), fields(db.system = "postgresql"))]
    async fn is_ready(&self) -> bool {
        match sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
        {
            Ok(1) => true,
            Ok(value) => {
                tracing::error!(
                    value,
                    "PostgreSQL readiness query returned an unexpected value"
                );
                false
            }
            Err(error) => {
                tracing::warn!(error = ?error, "PostgreSQL readiness check failed");
                false
            }
        }
    }
}
