use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::{Mutex, mpsc};
use tracing::{error, info};

use crate::config::loader;
use crate::ipc::{
    protocol::{LogEvent, LogLevel},
    session::BroadcastHub,
    socket_path::default_socket_path,
    unix_server::IpcServer,
};
use crate::obs::client::ObsClient;
use crate::runtime::shutdown;
use crate::server::{
    client_registry::ClientRegistry,
    command_executor::{CommandExecutor, CommandExecutorConfig},
    obs_supervisor::ObsSupervisor,
    options::ServerOptions,
    state_store::StateStore,
};

/// Start the daemon and block until shutdown.
/// Returns the process exit code (0 = success, 1 = startup failure).
pub async fn run(options: ServerOptions) -> i32 {
    // Load config
    let config_path = options
        .config_path
        .clone()
        .or_else(crate::config::paths::config_path)
        .unwrap_or_else(dirs_next_config_path);

    let config = match loader::load_or_default(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {e}");
            return 2;
        }
    };

    // Validate config
    if let Err(e) = crate::config::schema::validate(&config) {
        error!("Config invalid: {e}");
        return 2;
    }

    let socket_path = config
        .server
        .socket_path
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    // Remove stale socket file if present
    if socket_path.exists() {
        match try_remove_stale_socket(&socket_path).await {
            Ok(()) => {}
            Err(e) => {
                error!("Socket path occupied by live server: {e}");
                return 3;
            }
        }
    }

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let reconnecting = Arc::new(AtomicBool::new(false));
    let config_shared = Arc::new(Mutex::new(config));
    let registry = ClientRegistry::new();

    let (shutdown_tx, shutdown_rx) = shutdown::channel();
    let (reconnect_tx, reconnect_rx) = mpsc::channel::<()>(4);
    let (cmd_tx, cmd_rx) = mpsc::channel(128);

    // Bind IPC server
    let ipc_server =
        match IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry.clone()) {
            Ok(s) => s,
            Err(e) => {
                error!(
                    "Failed to bind IPC socket at {}: {e}",
                    socket_path.display()
                );
                return 3;
            }
        };
    info!("IPC server listening at {}", socket_path.display());
    hub.publish_log(
        LogEvent::new(
            LogLevel::Info,
            format!("IPC server listening at {}", socket_path.display()),
        )
        .with_target("obsctl_rs::server::daemon"),
    );

    // Install OS signal handlers
    shutdown::install_signal_handler(shutdown_tx.clone());

    let executor = CommandExecutor::new(CommandExecutorConfig {
        state: state.clone(),
        obs: Arc::clone(&obs_handle),
        config: Arc::clone(&config_shared),
        config_path: Some(config_path.clone()),
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx: reconnect_tx.clone(),
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });

    let supervisor = ObsSupervisor::new(
        Arc::clone(&config_shared),
        state.clone(),
        Arc::clone(&obs_handle),
        Arc::clone(&reconnecting),
        reconnect_rx,
        shutdown_rx.clone(),
        Arc::clone(&hub),
    );

    // Spawn tasks
    let _executor_handle = tokio::spawn(executor.run(cmd_rx));
    let supervisor_handle = tokio::spawn(supervisor.run());

    // Run accept loop until shutdown
    ipc_server.run(cmd_tx, shutdown_rx).await;
    info!("IPC accept loop stopped");
    hub.publish_log(
        LogEvent::new(LogLevel::Info, "IPC accept loop stopped")
            .with_target("obsctl_rs::server::daemon"),
    );

    // Wait for supervisor
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), supervisor_handle).await;

    // Cleanup socket
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    info!("obsctl server shutdown complete");
    hub.publish_log(
        LogEvent::new(LogLevel::Info, "obsctl server shutdown complete")
            .with_target("obsctl_rs::server::daemon"),
    );
    0
}

/// Attempt to connect to an existing socket to verify it is alive.
/// If it responds, the socket is live and we should not replace it.
/// If it cannot connect or times out, we remove the stale file.
async fn try_remove_stale_socket(path: &PathBuf) -> Result<(), String> {
    use tokio::net::UnixStream;

    match tokio::time::timeout(
        std::time::Duration::from_millis(500),
        UnixStream::connect(path),
    )
    .await
    {
        Ok(Ok(_)) => Err(format!(
            "live server is already running at {}",
            path.display()
        )),
        _ => {
            // No response → stale socket
            std::fs::remove_file(path).map_err(|e| format!("failed to remove stale socket: {e}"))
        }
    }
}

fn dirs_next_config_path() -> PathBuf {
    crate::config::paths::default_config_path()
        .unwrap_or_else(|| PathBuf::from("/tmp/obsctl_config.yml"))
}
