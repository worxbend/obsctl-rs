use tokio::sync::watch;

/// Create a shutdown channel. The sender is used to trigger shutdown;
/// the receiver is cloned and passed to tasks that need to observe it.
pub fn channel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Install a SIGTERM/SIGINT handler that fires the given shutdown sender.
pub fn install_signal_handler(tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
        let _ = tx.send(true);
    });
}
