use std::time::Duration;

use tokio::sync::watch;

pub fn channel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

pub fn requested(receiver: &watch::Receiver<bool>) -> bool {
    *receiver.borrow()
}

pub async fn wait(receiver: &mut watch::Receiver<bool>) {
    if requested(receiver) {
        return;
    }

    while receiver.changed().await.is_ok() {
        if requested(receiver) {
            return;
        }
    }
}

pub async fn wait_or_timeout(receiver: &mut watch::Receiver<bool>, duration: Duration) -> bool {
    tokio::select! {
        () = wait(receiver) => true,
        () = tokio::time::sleep(duration) => false,
    }
}

pub async fn notify_on_signal(sender: watch::Sender<bool>) {
    os_signal().await;
    tracing::info!("shutdown signal received");
    let _ = sender.send(true);
}

async fn os_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
