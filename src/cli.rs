use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "base_skeleton_rust",
    version,
    about = "HTTP server, job worker, and database operations"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Start the REST API server.
    Http,
    /// Start the PostgreSQL background-job worker.
    Worker,
    /// Start HTTP and worker in one process. Intended for local or simple deployments.
    All {
        /// Apply pending migrations before starting either component.
        #[arg(long)]
        migrate: bool,
    },
    /// Run a database migration operation and exit.
    Db {
        #[command(subcommand)]
        command: DatabaseCommand,
    },
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum DatabaseCommand {
    /// Apply all pending migrations.
    Migrate,
    /// Display embedded migration status.
    Info,
    /// Revert the latest migration when it has a matching down migration.
    #[command(alias = "undo")]
    Revert {
        /// Confirm the destructive schema operation.
        #[arg(long)]
        yes: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runtime_commands() {
        assert_eq!(
            Cli::try_parse_from(["app", "http"]).unwrap().command,
            Command::Http
        );
        assert_eq!(
            Cli::try_parse_from(["app", "all", "--migrate"])
                .unwrap()
                .command,
            Command::All { migrate: true }
        );
    }

    #[test]
    fn accepts_undo_as_a_revert_alias() {
        assert_eq!(
            Cli::try_parse_from(["app", "db", "undo", "--yes"])
                .unwrap()
                .command,
            Command::Db {
                command: DatabaseCommand::Revert { yes: true }
            }
        );
    }
}
