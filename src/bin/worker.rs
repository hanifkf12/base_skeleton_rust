use base_skeleton_rust::bootstrap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::run_worker().await
}
