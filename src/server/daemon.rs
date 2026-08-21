use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::{Mutex, mpsc};
use tracing::{error, info, warn};

use crate::config::loader;
use crate::domain::errors::ObsctlError;
use crate::ipc::{
    protocol::{LogEvent, LogLevel},
    session::BroadcastHub,
    socket_path,
    unix_server::IpcServer,
};
use crate::obs::client::ObsClient;
use crate::runtime::shutdown;
use crate::server::{
    client_registry::ClientRegistry,
    command_executor::{CommandExecutor, CommandExecutorConfig},
    command_lanes::ExecutorLanes,
    obs_supervisor::{ObsSupervisor, ObsSupervisorConfig},
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

    let (config, socket_path, _) = match loader::load_or_default_with_runtime(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return startup_failure(
                format!("Failed to load config: {e}"),
                ObsctlError::ConfigInvalid(e.to_string()),
            );
        }
    };

    if let Err(message) = socket_path::prepare(&socket_path).await {
        return startup_failure(&message, ObsctlError::IpcConnectionFailed(message.clone()));
    }

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let reconnecting = Arc::new(AtomicBool::new(false));
    let config_shared = Arc::new(Mutex::new(config));
    let registry = ClientRegistry::new();

    let (shutdown_tx, shutdown_rx) = shutdown::channel();
    let (reconnect_tx, reconnect_rx) = mpsc::channel::<()>(4);

    // Bind IPC server
    let ipc_server =
        match IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry.clone()) {
            Ok(s) => s,
            Err(e) => {
                return startup_failure(
                    format!(
                        "Failed to bind IPC socket at {}: {e}",
                        socket_path.display()
                    ),
                    ObsctlError::IpcConnectionFailed(e.to_string()),
                );
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

    let supervisor = ObsSupervisor::new(ObsSupervisorConfig {
        config: Arc::clone(&config_shared),
        state: state.clone(),
        obs_handle: Arc::clone(&obs_handle),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_rx,
        shutdown: shutdown_rx.clone(),
        hub: Arc::clone(&hub),
    });

    // One executor, reached through one lane per connection: commands from a
    // single client keep the order that client sent them in, and a slow command
    // from one client no longer holds up every other client's.
    let lanes = Arc::new(ExecutorLanes::new(executor));

    // Spawn tasks
    let supervisor_handle = tokio::spawn(supervisor.run());

    // Run accept loop until shutdown
    ipc_server.run(Arc::clone(&lanes), shutdown_rx).await;
    announce(&hub, "IPC accept loop stopped");

    // Nothing opens a lane once the accept loop has stopped, so from here the
    // set of lanes to wait for is fixed. The wait is spawned only because the
    // tidy-up below puts one deadline on both it and the supervisor.
    let lanes_handle = tokio::spawn(async move { lanes.drain().await });

    // The exit code stays 0 whether or not every step of the shutdown managed
    // to finish: the daemon was asked to stop and it stopped. An incomplete
    // tidy-up is something to read about in the log, not a failed exit that
    // would have systemd restart the service.
    let _completed = shut_down(&hub, &socket_path, lanes_handle, supervisor_handle).await;
    0
}

/// Report a failure that stops the daemon before it has started, and give back
/// the process exit code for it.
///
/// `message` is what the operator reads. `classification` says which kind of
/// local failure this is, and nothing else: splitting the two is what lets the
/// exit code come from `ObsctlError::exit_code` — the table `CLAUDE.md` calls
/// normative for local failures, and the one the README's Exit Codes table
/// documents — rather than from the bare `2` and `3` that used to be written
/// out here by hand, where nothing tied them to that table or would have
/// noticed them drifting away from it.
fn startup_failure(message: impl AsRef<str>, classification: ObsctlError) -> i32 {
    error!("{}", message.as_ref());
    classification.exit_code()
}

/// How long the daemon's background tasks get to finish once the accept loop
/// has stopped.
///
/// It is one budget shared by both tasks rather than five seconds each, so how
/// long a shutdown can take does not grow with the number of tasks.
///
/// Five seconds is less than `reconnect.max_delay_ms`, which defaults to ten:
/// a supervisor parked in its longest backoff sleep can be waiting longer than
/// this. It normally wakes anyway, because that sleep races the shutdown
/// signal, but the two numbers are not arranged so that it must — which is why
/// running out of this budget is reported rather than passed over in silence.
const TASK_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Stop the daemon's background work and remove the socket file, reporting
/// whether every step actually finished.
///
/// The answer matters because the last thing the daemon says about itself is
/// built from it. The old code awaited nothing, discarded the timeout result,
/// and then announced "shutdown complete" unconditionally — so a command still
/// waiting on OBS and a supervisor that never stopped both produced exactly the
/// same closing line as a clean stop.
async fn shut_down(
    hub: &BroadcastHub,
    socket_path: &Path,
    lanes_handle: tokio::task::JoinHandle<()>,
    supervisor_handle: tokio::task::JoinHandle<()>,
) -> bool {
    let deadline = tokio::time::Instant::now() + TASK_SHUTDOWN_GRACE;

    // The command lanes are drained first, and awaited rather than abandoned.
    // A lane's receiver closes by itself once its connection has gone and
    // dropped the sender, so waiting here is not waiting to be told to stop —
    // it is giving the commands already in flight time to finish their OBS
    // requests and answer. Dropping the handle instead cut them off
    // mid-request, and the client that sent one got "command handler dropped"
    // instead of a result.
    let lanes_stopped = join_by(hub, deadline, "command lanes", lanes_handle).await;
    let supervisor_stopped = join_by(hub, deadline, "OBS supervisor", supervisor_handle).await;

    let socket_removed = match socket_path::cleanup(socket_path) {
        Ok(()) => true,
        Err(err) => {
            report_problem(
                hub,
                format!(
                    "Failed to remove IPC socket file {}: {err}",
                    socket_path.display()
                ),
            );
            false
        }
    };

    let completed = lanes_stopped && supervisor_stopped && socket_removed;
    if completed {
        announce(hub, "obsctl server shutdown complete");
    } else {
        report_problem(hub, "obsctl server shutdown finished with steps incomplete");
    }
    completed
}

/// Wait for one background task to finish before `deadline`, reporting whether
/// it did and naming it if it did not.
///
/// The name is the point: "a task did not stop in time" is not something the
/// next person to read the log can act on, and the two tasks fail for entirely
/// different reasons.
async fn join_by(
    hub: &BroadcastHub,
    deadline: tokio::time::Instant,
    task: &str,
    handle: tokio::task::JoinHandle<()>,
) -> bool {
    match tokio::time::timeout_at(deadline, handle).await {
        Ok(Ok(())) => true,
        Ok(Err(err)) => {
            report_problem(hub, format!("The {task} task ended abnormally: {err}"));
            false
        }
        Err(_) => {
            report_problem(
                hub,
                format!("The {task} did not stop within {TASK_SHUTDOWN_GRACE:?}"),
            );
            false
        }
    }
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

/// [`announce`] for something that went wrong, so a problem reaches a watching
/// client's log pane by the same route a milestone does instead of only
/// existing in the process log the operator may not be looking at.
fn report_problem(hub: &BroadcastHub, message: impl Into<String>) {
    let message = message.into();
    warn!("{message}");
    hub.publish_log(
        LogEvent::new(LogLevel::Warn, message).with_target("obsctl_rs::server::daemon"),
    );
}

fn dirs_next_config_path() -> PathBuf {
    crate::config::paths::default_config_path()
        .unwrap_or_else(|| PathBuf::from("/tmp/obsctl_config.yml"))
}
