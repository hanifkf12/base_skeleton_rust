use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use sqlx::{FromRow, migrate::Migrator, postgres::PgPoolOptions};

use crate::{cli::DatabaseCommand, config::Config};

static MIGRATOR: Migrator = sqlx::migrate!();

#[derive(FromRow)]
struct AppliedMigrationRow {
    version: i64,
    success: bool,
    checksum: Vec<u8>,
}

pub async fn run(command: DatabaseCommand) -> Result<()> {
    match command {
        DatabaseCommand::Migrate => migrate().await,
        DatabaseCommand::Info => info().await,
        DatabaseCommand::Revert { yes } => revert(yes).await,
    }
}

pub async fn migrate() -> Result<()> {
    let pool = connect().await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("could not run PostgreSQL migrations")?;
    tracing::info!("database migrations are up to date");
    Ok(())
}

async fn info() -> Result<()> {
    let pool = connect().await?;
    let table_exists = migration_table_exists(&pool).await?;

    let applied = if table_exists {
        sqlx::query_as::<_, AppliedMigrationRow>(
            "SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version",
        )
        .fetch_all(&pool)
        .await
        .context("could not read applied PostgreSQL migrations")?
        .into_iter()
        .map(|migration| (migration.version, migration))
        .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };

    println!("VERSION\tSTATUS\tDESCRIPTION");
    let mut invalid = false;
    let known_versions = MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .map(|migration| migration.version)
        .collect::<HashSet<_>>();
    for migration in MIGRATOR
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
    {
        let status = match applied.get(&migration.version) {
            Some(row) if !row.success => {
                invalid = true;
                "failed"
            }
            Some(row) if row.checksum.as_slice() != migration.checksum.as_ref() => {
                invalid = true;
                "checksum_mismatch"
            }
            Some(_) => "applied",
            None => "pending",
        };
        println!(
            "{}\t{}\t{}",
            migration.version, status, migration.description
        );
    }
    let mut missing_versions = applied
        .values()
        .filter(|row| !known_versions.contains(&row.version))
        .map(|row| row.version)
        .collect::<Vec<_>>();
    missing_versions.sort_unstable();
    for version in missing_versions {
        invalid = true;
        println!("{version}\tmissing_from_binary\tunknown");
    }

    if invalid {
        bail!("database migration state contains a failure or checksum mismatch");
    }
    Ok(())
}

async fn revert(confirmed: bool) -> Result<()> {
    if !confirmed {
        bail!("db revert is destructive; rerun with --yes after reviewing the down migration");
    }

    let pool = connect().await?;
    if !migration_table_exists(&pool).await? {
        bail!("there is no applied migration to revert");
    }
    let applied_versions = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version DESC",
    )
    .fetch_all(&pool)
    .await
    .context("could not read applied PostgreSQL migrations")?;
    let Some(latest_version) = applied_versions.first().copied() else {
        bail!("there is no applied migration to revert");
    };

    let reversible = MIGRATOR.iter().any(|migration| {
        migration.version == latest_version && migration.migration_type.is_down_migration()
    });
    if !reversible {
        bail!(
            "migration {latest_version} is forward-only and cannot be reverted; create a corrective migration"
        );
    }

    let target = applied_versions.get(1).copied().unwrap_or(0);
    MIGRATOR
        .undo(&pool, target)
        .await
        .with_context(|| format!("could not revert migration {latest_version}"))?;
    tracing::info!(version = latest_version, "database migration reverted");
    Ok(())
}

async fn connect() -> Result<sqlx::PgPool> {
    let database_url = Config::migration_database_url_from_env()?;
    PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .context("could not connect migration command to PostgreSQL")
}

async fn migration_table_exists(pool: &sqlx::PgPool) -> Result<bool> {
    sqlx::query_scalar("SELECT to_regclass(current_schema() || '._sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await
        .context("could not inspect the PostgreSQL migration table")
}
