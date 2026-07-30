use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use sqlx::{FromRow, postgres::PgPoolOptions};

use crate::{
    cli::DatabaseCommand,
    config::Config,
    infrastructure::database::postgres::migrations::{MIGRATOR, migration_table_exists},
};

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

pub fn create_migration(name: &str) -> Result<()> {
    let migrations_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let version = Utc::now().format("%Y%m%d%H%M%S").to_string().parse()?;
    let path = create_migration_file(&migrations_dir, name, version)?;
    println!("created {}", path.display());
    Ok(())
}

fn create_migration_file(migrations_dir: &Path, name: &str, version: i64) -> Result<PathBuf> {
    validate_migration_name(name)?;
    ensure!(
        migrations_dir.is_dir(),
        "migration directory {} does not exist",
        migrations_dir.display()
    );

    let next_version = next_migration_version(migrations_dir, version)?;
    let path = migrations_dir.join(format!("{next_version}_{name}.sql"));
    fs::write(&path, "-- Write forward-only schema changes here.\n")
        .with_context(|| format!("could not create migration file {}", path.display()))?;
    Ok(path)
}

fn validate_migration_name(name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "migration name must not be empty");
    ensure!(
        name.len() <= 64,
        "migration name must contain at most 64 characters"
    );
    ensure!(
        name.bytes().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == b'_'
        }),
        "migration name must use snake_case lowercase ASCII letters, digits, and underscores"
    );
    ensure!(
        !name.starts_with('_') && !name.ends_with('_') && !name.contains("__"),
        "migration name must use valid snake_case"
    );
    Ok(())
}

fn next_migration_version(migrations_dir: &Path, version: i64) -> Result<i64> {
    let mut next_version = version;
    for entry in fs::read_dir(migrations_dir).with_context(|| {
        format!(
            "could not read migration directory {}",
            migrations_dir.display()
        )
    })? {
        let entry = entry.context("could not read migration directory entry")?;
        let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((prefix, _)) = file_name.split_once('_') else {
            continue;
        };
        let Ok(existing_version) = prefix.parse::<i64>() else {
            continue;
        };
        if existing_version >= next_version {
            next_version = existing_version
                .checked_add(1)
                .context("migration version overflow")?;
        }
    }
    Ok(next_version)
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

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{create_migration_file, validate_migration_name};

    fn temporary_migrations_dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("base_skeleton_migrations_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn creates_a_timestamped_forward_only_migration() {
        let migrations_dir = temporary_migrations_dir();
        let path =
            create_migration_file(&migrations_dir, "add_users_status", 20_260_730_123_456).unwrap();

        assert_eq!(
            path.file_name().unwrap(),
            "20260730123456_add_users_status.sql"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "-- Write forward-only schema changes here.\n"
        );
        fs::remove_dir_all(migrations_dir).unwrap();
    }

    #[test]
    fn increments_a_conflicting_migration_version() {
        let migrations_dir = temporary_migrations_dir();
        fs::write(
            migrations_dir.join("20260730123456_existing.sql"),
            "-- existing\n",
        )
        .unwrap();

        let path =
            create_migration_file(&migrations_dir, "add_users_status", 20_260_730_123_456).unwrap();

        assert_eq!(
            path.file_name().unwrap(),
            "20260730123457_add_users_status.sql"
        );
        fs::remove_dir_all(migrations_dir).unwrap();
    }

    #[test]
    fn rejects_non_snake_case_migration_names() {
        assert!(validate_migration_name("AddUsersStatus").is_err());
        assert!(validate_migration_name("add-users-status").is_err());
        assert!(validate_migration_name("_add_users_status").is_err());
        assert!(validate_migration_name("add__users_status").is_err());
    }
}
