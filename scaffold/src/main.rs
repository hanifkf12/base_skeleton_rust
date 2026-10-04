use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use skeleton_new::{ProjectName, Source, format_project, generate};

const DEFAULT_REPO: &str = "https://github.com/hanifkf12/base_skeleton_rust.git";

const EXAMPLES: &str = "\
Examples:
  skeleton-new orders-api                        Create ./orders-api
  skeleton-new orders-api --path ~/work          Create ~/work/orders-api
  skeleton-new orders-api --branch v2            Use a branch or tag of the skeleton
  skeleton-new orders-api --repo git@github.com:me/skeleton.git
                                                 Use another skeleton repository

Commands:
  skeleton-new help                              Show this help (same as --help)

The name `help` is reserved for the command above.";

/// Generate a new service from the Rust/Axum clean-architecture skeleton.
///
/// The skeleton is fetched from git on every run, so a new project always starts
/// from the latest committed version. Requires `git` and `cargo` on PATH.
#[derive(Debug, Parser)]
#[command(
    name = "skeleton-new",
    version,
    arg_required_else_help = true,
    after_help = EXAMPLES
)]
struct Args {
    /// Project name: lowercase letters, digits, '-' and '_' (for example `orders-api`).
    /// It becomes the directory, crate, binary, Docker image and database name.
    name: String,

    /// Directory in which the project directory is created.
    #[arg(long, short, default_value = ".")]
    path: PathBuf,

    /// Git repository (URL or local path) that holds the skeleton.
    #[arg(long, env = "SKELETON_REPO", default_value = DEFAULT_REPO)]
    repo: String,

    /// Branch or tag to fetch instead of the repository's default branch.
    #[arg(long, short, env = "SKELETON_BRANCH")]
    branch: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    // `skeleton-new help` shows the help instead of creating a project named "help".
    if args.name == "help" {
        Args::command().print_long_help()?;
        return Ok(());
    }
    let name = ProjectName::parse(&args.name)?;
    let source = Source {
        repo: &args.repo,
        reference: args.branch.as_deref(),
    };
    let generated = generate(&name, &args.path, source)?;
    format_project(&generated.directory).context(
        "the project was generated but could not be formatted; run `cargo fmt` inside it",
    )?;

    println!(
        "Created `{}` ({} files) from {}@{} at {}",
        name.as_str(),
        generated.files,
        args.repo,
        generated.revision,
        generated.directory.display()
    );
    println!();
    println!("Next steps (details: README.md, \"First-time setup\"):");
    println!("  cd {}", generated.directory.display());
    println!(
        "  git init -b main && git add -A && git commit -m \"chore: initial commit from skeleton\""
    );
    println!(
        "  cp .env.example .env   # defaults work with docker compose; see README for a real OIDC provider"
    );
    println!("  docker compose up -d");
    println!("  cargo run -- db migrate");
    println!("  cargo run -- all");
    Ok(())
}
