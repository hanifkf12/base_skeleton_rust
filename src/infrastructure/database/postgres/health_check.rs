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
        let result = sqlx::query_scalar::<_, i64>(
            r#"SELECT COUNT(*)
               FROM information_schema.tables
               WHERE table_schema = 'public'
                 AND table_name IN ('users', 'background_jobs')"#,
        )
        .fetch_one(&self.pool)
        .await;

        match result {
            Ok(2) => true,
            Ok(count) => {
                tracing::warn!(
                    tables_found = count,
                    "PostgreSQL readiness check: expected 2 required tables, found {count}"
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
