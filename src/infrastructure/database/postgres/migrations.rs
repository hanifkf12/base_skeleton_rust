use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::{FromRow, PgPool, migrate::Migrator};

pub static MIGRATOR: Migrator = sqlx::migrate!();

#[derive(FromRow)]
pub struct AppliedMigration {
    pub version: i64,
    pub success: bool,
    pub checksum: Vec<u8>,
}

pub async fn load_applied_migrations(pool: &PgPool) -> Result<HashMap<i64, AppliedMigration>> {
    if !migration_table_exists(pool).await? {
        return Ok(HashMap::new());
    }

    Ok(sqlx::query_as::<_, AppliedMigration>(
        "SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .context("could not read applied PostgreSQL migrations")?
    .into_iter()
    .map(|migration| (migration.version, migration))
    .collect())
}

pub async fn migration_state_is_current(pool: &PgPool) -> Result<bool> {
    let applied = load_applied_migrations(pool).await?;

    Ok(!applied.is_empty()
        && applied.values().all(|row| row.success)
        && MIGRATOR
            .iter()
            .filter(|migration| migration.migration_type.is_up_migration())
            .all(|migration| {
                applied.get(&migration.version).is_some_and(|row| {
                    row.success && row.checksum.as_slice() == migration.checksum.as_ref()
                })
            }))
}

pub async fn migration_table_exists(pool: &PgPool) -> Result<bool> {
    sqlx::query_scalar("SELECT to_regclass(current_schema() || '._sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await
        .context("could not inspect the PostgreSQL migration table")
}
