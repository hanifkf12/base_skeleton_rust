use async_trait::async_trait;
use sqlx::PgPool;

use super::migrations;
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
        let result = migrations::migration_state_is_current(&self.pool).await;

        match result {
            Ok(true) => true,
            Ok(false) => {
                tracing::warn!("PostgreSQL migration state does not match the application binary");
                false
            }
            Err(error) => {
                tracing::warn!(error = ?error, "PostgreSQL readiness check failed");
                false
            }
        }
    }
}
