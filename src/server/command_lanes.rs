use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::warn;

use crate::ipc::session::{CommandDispatch, CommandLanes};
use crate::server::command_executor::CommandExecutor;

/// How many commands one connection may have waiting behind the command of
/// its own that is currently running.
///
/// This is what keeps a client from queueing an unbounded amount of work: once
/// a connection has this many commands waiting, the task reading that
/// connection's socket blocks on the send, which stops reading it, which stops
/// the client — and only that client — from piling on more. The number is a
/// depth per connection rather than one shared budget, so a busy client cannot
/// use up the room another client needs.
const LANE_DEPTH: usize = 32;

/// Gives every IPC connection its own lane through one shared
/// [`CommandExecutor`].
///
/// The daemon used to hand every connection a clone of a single sender, drained
/// by one loop that ran a command to completion before it looked at the next.
/// That made *all* commands wait for the slowest one: while `dump-config` did
/// its two OBS round trips and rewrote the config file, a `ping` from any other
/// client sat in the queue for the whole of it.
///
/// A lane is one `tokio` task serving one connection's channel with exactly
/// that same run-to-completion loop, so the guarantee a single client relies on
/// — that `mute Mic` then `unmute Mic` happen in that order — is unchanged and
/// needs no bookkeeping to maintain: it is the shape of the loop. What changes
/// is that there is now one such loop per connection, and tasks run
/// concurrently, so an unrelated client's command is not behind anybody else's.
///
/// Nothing here weakens the daemon's ownership of OBS: every lane calls the
/// same executor, which holds the one OBS client, and obs-websocket already
/// correlates several requests in flight at once by request id.
pub struct ExecutorLanes {
    executor: Arc<CommandExecutor>,
    /// The tasks serving the lanes handed out so far, kept so that shutdown can
    /// wait for the commands still running in them.
    ///
    /// A plain (non-async) mutex is enough and is never held across an await:
    /// spawning into the set and taking the set out of it both finish without
    /// yielding.
    tasks: Mutex<JoinSet<()>>,
}

impl ExecutorLanes {
    pub fn new(executor: CommandExecutor) -> Self {
        Self {
            executor: Arc::new(executor),
            tasks: Mutex::new(JoinSet::new()),
        }
    }

    /// Wait for every lane opened so far to finish the commands it still has.
    ///
    /// A lane ends by itself when its connection goes away, because that drops
    /// the only sender its channel has. So this is not a way of telling lanes to
    /// stop — it is the daemon giving commands that are already running the
    /// chance to finish and answer before the process leaves. The caller puts a
    /// deadline on it; without one, a client that stays connected would keep the
    /// daemon here.
    ///
    /// Lanes opened after this starts are not waited for, which cannot happen
    /// in the daemon: it drains only once the accept loop — the only thing that
    /// opens lanes — has stopped.
    pub async fn drain(&self) {
        let mut tasks = std::mem::take(&mut *self.lock_tasks());
        while let Some(finished) = tasks.join_next().await {
            if let Err(error) = finished {
                warn!("A command lane ended abnormally: {error}");
            }
        }
    }

    /// A lane task can only panic if the executor does, and the executor
    /// catches its own failures and answers them as errors. Recovering the set
    /// from a poisoned mutex therefore loses nothing: the tasks in it are still
    /// joinable, and refusing to drain them would turn one panic into a daemon
    /// that cannot shut down.
    fn lock_tasks(&self) -> std::sync::MutexGuard<'_, JoinSet<()>> {
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CommandLanes for ExecutorLanes {
    fn open_lane(&self) -> mpsc::Sender<CommandDispatch> {
        let (tx, rx) = mpsc::channel(LANE_DEPTH);
        let executor = Arc::clone(&self.executor);
        self.lock_tasks()
            .spawn(async move { executor.serve(rx).await });
        tx
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    use serde_json::{Value, json};
    use tokio::sync::{Mutex as AsyncMutex, oneshot};

    use crate::config::model::Config;
    use crate::ipc::protocol::{CommandPayload, ServerMessage};
    use crate::ipc::session::BroadcastHub;
    use crate::server::client_registry::ClientRegistry;
    use crate::server::command_executor::CommandExecutorConfig;
    use crate::server::state_store::StateStore;

    use super::*;

    fn test_lanes() -> ExecutorLanes {
        let hub = Arc::new(BroadcastHub::new());
        let state = StateStore::new(Arc::clone(&hub));
        let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        ExecutorLanes::new(CommandExecutor::new(CommandExecutorConfig {
            state,
            obs: Arc::new(AsyncMutex::new(None)),
            config: Arc::new(AsyncMutex::new(Config::default())),
            config_path: None,
            socket_path: PathBuf::from("/tmp/obsctl-lanes-test.sock"),
            registry: ClientRegistry::new(),
            reconnecting: Arc::new(AtomicBool::new(false)),
            reconnect_tx,
            shutdown_tx,
            hub,
        }))
    }

    /// Drain with a deadline, so a lane that never ends fails the test with
    /// something to read instead of hanging it.
    async fn drain_within(lanes: &ExecutorLanes, what: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(5), lanes.drain())
            .await
            .unwrap_or_else(|_| panic!("draining {what} should not block"));
    }

    async fn ping(lane: &mpsc::Sender<CommandDispatch>, id: &str) -> Value {
        let (reply, answer) = oneshot::channel();
        lane.send(CommandDispatch {
            id: id.to_string(),
            payload: CommandPayload {
                name: "ping".to_string(),
                args: Value::Null,
            },
            reply,
        })
        .await
        .expect("the lane should accept a command");

        match answer.await.expect("the lane should answer") {
            ServerMessage::Response { id, result, .. } => json!({ "id": id, "result": result }),
            other => panic!("expected a response, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn each_session_gets_its_own_lane() {
        let lanes = test_lanes();

        let first = lanes.open_lane();
        let second = lanes.open_lane();

        assert_eq!(ping(&first, "a").await["id"], "a");
        assert_eq!(ping(&second, "b").await["id"], "b");
    }

    #[tokio::test]
    async fn a_lane_ends_when_its_session_hangs_up() {
        let lanes = test_lanes();

        let lane = lanes.open_lane();
        assert_eq!(ping(&lane, "a").await["result"]["message"], "pong");

        // Dropping the sender is what a closed connection does; the lane task
        // then finishes on its own, so draining returns rather than blocking.
        drop(lane);
        drain_within(&lanes, "a lane whose session hung up").await;
    }

    #[tokio::test]
    async fn draining_waits_for_a_command_already_in_the_lane() {
        let lanes = test_lanes();
        let lane = lanes.open_lane();

        let (reply, mut answer) = oneshot::channel();
        lane.send(CommandDispatch {
            id: "queued".to_string(),
            payload: CommandPayload {
                name: "ping".to_string(),
                args: Value::Null,
            },
            reply,
        })
        .await
        .unwrap();
        drop(lane);

        drain_within(&lanes, "a lane with one command queued").await;

        // The answer is already waiting: the drain did not return until the
        // lane had finished the command it had been given.
        match answer
            .try_recv()
            .expect("the queued command should have run")
        {
            ServerMessage::Response { id, ok, .. } => {
                assert_eq!(id, "queued");
                assert!(ok);
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }
}
