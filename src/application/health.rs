use async_trait::async_trait;

/// Reports whether required dependencies can currently serve application traffic.
#[async_trait]
pub trait ReadinessCheck: Send + Sync {
    async fn is_ready(&self) -> bool;
}
