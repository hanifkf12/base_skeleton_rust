use base_skeleton_rust::{bootstrap, cli::Cli, telemetry};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let telemetry = telemetry::init()?;
    let result = bootstrap::run(Cli::parse().command).await;
    telemetry.shutdown();
    result
}
