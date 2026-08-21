use tokio::sync::watch;

/// Create a shutdown channel. The sender is used to trigger shutdown;
/// the receiver is cloned and passed to tasks that need to observe it.
pub fn channel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Install a SIGTERM/SIGINT handler that fires the given shutdown sender.
pub fn install_signal_handler(tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        let sigterm = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            Ok(sigterm) => Some(sigterm),
            Err(error) => {
                // The daemon reports everything else through `tracing`, which
                // reaches both the log file and the `logs` IPC topic. Printing
                // straight to stderr from this spawned task put the one warning
                // about a missing signal handler somewhere nobody watching the
                // daemon would see it.
                tracing::warn!(%error, "failed to listen for SIGTERM");
                None
            }
        };

        match sigterm {
            Some(mut sigterm) => {
                tokio::select! {
                    _ = async {
                        if let Err(error) = tokio::signal::ctrl_c().await {
                            tracing::warn!(%error, "failed to listen for Ctrl-C");
                        }
                    } => {}
                    _ = sigterm.recv() => {}
                }
            }
            None => {
                if let Err(error) = tokio::signal::ctrl_c().await {
                    tracing::warn!(%error, "failed to listen for Ctrl-C");
                }
            }
        }

        let _ = tx.send(true);
    });
}
