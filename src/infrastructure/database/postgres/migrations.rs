use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use sqlx::{FromRow, PgPool, migrate::Migrator};

pub static MIGRATOR: Migrator = sqlx::migrate!();

#[derive(FromRow)]
struct AppliedMigration {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

pub async fn migration_state_is_current(pool: &PgPool) -> Result<bool> {
    if !migration_table_exists(pool).await? {
        return Ok(false);
    }

    let applied = sqlx::query_as::<_, AppliedMigration>(
        "SELECT version, success, checksum FROM _sqlx_migrations",
    )
    .fetch_all(pool)
    .await
    .context("could not read applied PostgreSQL migrations")?
    .into_iter()
    .map(|migration| (migration.version, migration))
    .collect::<HashMap<_, _>>();

    let expected = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .collect::<Vec<_>>();
    let expected_versions = expected
        .iter()
        .map(|migration| migration.version)
        .collect::<HashSet<_>>();

    Ok(applied.len() == expected.len()
        && applied
            .values()
            .all(|migration| expected_versions.contains(&migration.version))
        && expected.iter().all(|migration| {
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

pub async fn ensure_current(pool: &PgPool) -> Result<()> {
    ensure!(
        migration_state_is_current(pool).await?,
        "PostgreSQL migration state does not match this application binary"
    );
    Ok(())
}
