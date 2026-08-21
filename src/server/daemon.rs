use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt as _;

use tokio::sync::{Mutex, mpsc};
use tracing::{error, info, warn};

use crate::config::loader;
use crate::ipc::{
    protocol::{LogEvent, LogLevel},
    session::BroadcastHub,
    socket_path::{ensure_socket_file, validate_socket_path},
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
use crate::support::fs;

/// Start the daemon and block until shutdown.
/// Returns the process exit code (0 = success, 1 = startup failure).
pub async fn run(options: ServerOptions) -> i32 {
    // Load config
    let config_path = options
        .config_path
        .clone()
        .or_else(crate::config::paths::config_path)
        .unwrap_or_else(dirs_next_config_path);

    let (config, socket_path, _) = match loader::load_or_default_with_runtime(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {e}");
            return 2;
        }
    };

    if let Err(exit_code) = prepare_socket_path(&socket_path).await {
        return exit_code;
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
    announce(
        &hub,
        format!("IPC server listening at {}", socket_path.display()),
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
    announce(&hub, "IPC accept loop stopped");

    // Wait for supervisor
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), supervisor_handle).await;

    // Cleanup socket
    if let Err(err) = cleanup_socket_file(&socket_path) {
        warn!(
            "Failed to remove IPC socket file {}: {err}",
            socket_path.display()
        );
    }

    announce(&hub, "obsctl server shutdown complete");
    0
}

/// Report a daemon lifecycle milestone to both places it has to appear: the
/// process log, and the `logs` topic that connected clients watch.
///
/// Each of these used to be written out twice, once per destination, with the
/// message text repeated — so the operator's terminal and the TUI's log pane
/// could disagree about what happened after any edit that touched only one.
fn announce(hub: &BroadcastHub, message: impl Into<String>) {
    let message = message.into();
    info!("{message}");
    hub.publish_log(
        LogEvent::new(LogLevel::Info, message).with_target("obsctl_rs::server::daemon"),
    );
}

/// Make sure the socket path is one this daemon may bind, and that nothing is
/// already using it.
///
/// Split out of `run` because it is a distinct phase with a distinct outcome:
/// every failure here means "do not start", reported as the process exit code
/// the caller should return, before any of the daemon's moving parts exist.
async fn prepare_socket_path(socket_path: &Path) -> Result<(), i32> {
    const SOCKET_SETUP_FAILURE: i32 = 3;

    if let Err(err) = crate::ipc::socket_path::ensure_private_socket_parent(socket_path) {
        error!(
            "Refusing to use socket path {}: {err}",
            socket_path.display()
        );
        return Err(SOCKET_SETUP_FAILURE);
    }

    if let Err(error) = validate_socket_path(socket_path) {
        error!("Invalid socket path {}: {error}", socket_path.display());
        return Err(SOCKET_SETUP_FAILURE);
    }

    // A socket file left behind by a crashed daemon must be cleared, but one
    // belonging to a daemon that is still running must not be.
    if socket_path.exists()
        && let Err(e) = try_remove_stale_socket(socket_path).await
    {
        error!("Socket path occupied by live server: {e}");
        return Err(SOCKET_SETUP_FAILURE);
    }

    Ok(())
}

/// Attempt to connect to an existing socket to verify it is alive.
/// If it responds, the socket is live and we should not replace it.
/// If it cannot connect or times out, we remove the stale file.
async fn try_remove_stale_socket(path: &Path) -> Result<(), String> {
    validate_socket_path(path).map_err(|error| {
        if error.to_string().contains("not a private directory") {
            "socket parent is not private".to_string()
        } else {
            error.to_string()
        }
    })?;

    let parent = path
        .parent()
        .ok_or_else(|| "socket path has no parent".to_string())?;
    if fs::ensure_private_dir(parent).is_err() {
        return Err("socket parent is not private".to_string());
    }

    validate_socket_file_for_cleanup(path).map_err(|error| error.to_string())?;

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
            // Re-check the socket file immediately before deleting to avoid deleting a
            // newly replaced non-socket path under the same name.
            validate_socket_file_for_cleanup(path).map_err(|error| error.to_string())?;

            match fs::remove_with_type_guard(
                path,
                |metadata| metadata.file_type().is_socket(),
                "socket",
            ) {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(format!("failed to remove stale socket: {err}")),
            }
        }
    }
}

/// Why a path that should hold a live IPC socket cannot be treated as one.
///
/// The two cases are kept apart for `cleanup_socket_file`, which tolerates a
/// `File` error meaning "already gone" but not a `Path` error meaning "this is
/// not a path we are willing to touch". Callers that only need to report the
/// problem use the `Display` impl.
enum SocketFileValidation {
    Path(std::io::Error),
    File(std::io::Error),
}

impl std::fmt::Display for SocketFileValidation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(error) | Self::File(error) => error.fmt(f),
        }
    }
}

fn validate_socket_file_for_cleanup(path: &Path) -> Result<(), SocketFileValidation> {
    validate_socket_path(path).map_err(SocketFileValidation::Path)?;
    ensure_socket_file(path).map_err(SocketFileValidation::File)?;
    Ok(())
}

fn dirs_next_config_path() -> PathBuf {
    crate::config::paths::default_config_path()
        .unwrap_or_else(|| PathBuf::from("/tmp/obsctl_config.yml"))
}

fn cleanup_socket_file(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::ensure_private_dir(parent)?;
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "socket path has no parent",
        ));
    }

    match validate_socket_file_for_cleanup(path) {
        Ok(()) => {}
        Err(SocketFileValidation::Path(error)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid socket path for cleanup: {error}"),
            ));
        }
        Err(SocketFileValidation::File(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(SocketFileValidation::File(error)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid socket file for cleanup: {error}"),
            ));
        }
    }

    match fs::remove_with_type_guard(path, |metadata| metadata.file_type().is_socket(), "socket") {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::{cleanup_socket_file, try_remove_stale_socket};

    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::{NamedTempFile, tempdir};

    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(unix)]
    use tokio::net::UnixListener;

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_check_rejects_symlink_paths() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.sock");
        let link = dir.path().join("link.sock");
        symlink(&real, &link).unwrap();

        let result = try_remove_stale_socket(&link).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "socket path is symbolic link");
    }

    #[tokio::test]
    async fn stale_socket_check_rejects_non_socket_files() {
        let dir = tempdir().unwrap();
        let file = NamedTempFile::new_in(dir.path()).unwrap();
        let path = file.path().to_path_buf();
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        out.write_all(b"not a socket").unwrap();

        let result = try_remove_stale_socket(&path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a socket"));
        assert!(path.exists());
    }

    #[tokio::test]
    async fn stale_socket_check_rejects_relative_paths() {
        let path = PathBuf::from("relative.sock");
        let result = try_remove_stale_socket(&path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_check_removes_stale_listener_socket() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stale.sock");

        let listener = UnixListener::bind(&path).unwrap();
        drop(listener);

        let result = try_remove_stale_socket(&path).await;
        assert!(result.is_ok());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_check_rejects_unsafe_socket_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let parent = dir.path().join("unsafe-parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = parent.join("obsctl.sock");
        std::fs::write(&path, b"payload").unwrap();

        let error = try_remove_stale_socket(&path).await.unwrap_err();
        assert!(error.contains("socket parent is not private"));
        assert!(path.exists());
    }

    #[test]
    fn cleanup_socket_file_rejects_non_socket_paths() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not-a-socket.txt");
        std::fs::write(&path, b"payload").unwrap();

        let error = cleanup_socket_file(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("not a socket"));
        assert!(path.exists());
    }

    #[test]
    fn cleanup_socket_file_noop_when_path_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.sock");

        cleanup_socket_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_socket_file_rejects_broken_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let path = dir.path().join("link.sock");
        let target = dir.path().join("missing.sock");
        symlink(&target, &path).unwrap();

        let error = cleanup_socket_file(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("invalid socket path for cleanup"));
        assert!(std::fs::symlink_metadata(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_socket_file_rejects_unsafe_socket_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let parent = dir.path().join("unsafe-parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = parent.join("obsctl.sock");
        std::fs::write(&path, b"payload").unwrap();

        let error = cleanup_socket_file(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("not a private directory"));
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_socket_file_removes_stale_socket() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stale.sock");

        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        drop(listener);

        cleanup_socket_file(&path).unwrap();
        assert!(!path.exists());
    }
}
