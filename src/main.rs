use base_skeleton_rust::{bootstrap, cli::Cli};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run(Cli::parse().command).await
}
