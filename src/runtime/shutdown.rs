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
                eprintln!("warning: failed to listen for SIGTERM: {error}");
                None
            }
        };

        match sigterm {
            Some(mut sigterm) => {
                tokio::select! {
                    _ = async {
                        if let Err(error) = tokio::signal::ctrl_c().await {
                            eprintln!("warning: failed to listen for Ctrl-C: {error}");
                        }
                    } => {}
                    _ = sigterm.recv() => {}
                }
            }
            None => {
                if let Err(error) = tokio::signal::ctrl_c().await {
                    eprintln!("warning: failed to listen for Ctrl-C: {error}");
                }
            }
        }

        let _ = tx.send(true);
    });
}
