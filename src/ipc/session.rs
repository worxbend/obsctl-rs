use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::ipc::protocol::{CommandPayload, LogEvent, ObsEventPayload, ServerMessage, Topic};

pub const BROADCAST_CAPACITY: usize = 64;

/// Per-topic broadcast channels shared across all IPC client sessions.
#[derive(Debug, Clone)]
pub struct BroadcastHub {
    state_tx: broadcast::Sender<ServerMessage>,
    events_tx: broadcast::Sender<ServerMessage>,
    logs_tx: broadcast::Sender<ServerMessage>,
}

impl Default for BroadcastHub {
    fn default() -> Self {
        Self::new()
    }
}

impl BroadcastHub {
    pub fn new() -> Self {
        let (state_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (events_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (logs_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            state_tx,
            events_tx,
            logs_tx,
        }
    }

    pub fn publish(&self, topic: Topic, msg: ServerMessage) {
        let _ = match topic {
            Topic::State => self.state_tx.send(msg),
            Topic::Events => self.events_tx.send(msg),
            Topic::Logs => self.logs_tx.send(msg),
        };
    }

    pub fn publish_log(&self, event: LogEvent) {
        self.publish(Topic::Logs, ServerMessage::log_event(event));
    }

    pub fn publish_obs_event(&self, event: ObsEventPayload) {
        self.publish(Topic::Events, ServerMessage::obs_event(event));
    }

    /// Start receiving everything published on one topic from now on.
    ///
    /// The mirror image of `publish`: both pick the channel from the same
    /// `Topic` value, so a new topic cannot be published to without also
    /// being subscribable.
    pub fn subscribe(&self, topic: Topic) -> broadcast::Receiver<ServerMessage> {
        match topic {
            Topic::State => self.state_tx.subscribe(),
            Topic::Events => self.events_tx.subscribe(),
            Topic::Logs => self.logs_tx.subscribe(),
        }
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<ServerMessage> {
        self.subscribe(Topic::State)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerMessage> {
        self.subscribe(Topic::Events)
    }

    pub fn subscribe_logs(&self) -> broadcast::Receiver<ServerMessage> {
        self.subscribe(Topic::Logs)
    }
}

/// Dispatched from an IPC session to the server's command executor.
pub struct CommandDispatch {
    pub id: String,
    pub payload: CommandPayload,
    pub reply: oneshot::Sender<ServerMessage>,
}

/// Where a newly accepted IPC session gets the channel it dispatches on.
///
/// One connection's commands all travel down the one sender it is handed here,
/// and a channel delivers what is sent into it in order — so whoever is at the
/// receiving end of that sender decides the ordering rule for that connection,
/// and only for that connection. That is the whole point of handing out a
/// sender per session rather than sharing one: the daemon (see
/// `server::command_lanes`) gives each session a sender whose receiver is
/// served by its own task, which makes one client's commands run in the order
/// it sent them while a slow command from another client is running at the
/// same time.
pub trait CommandLanes: Send + Sync {
    /// Give one session the sender it will dispatch every command on.
    fn open_lane(&self) -> mpsc::Sender<CommandDispatch>;
}

/// A single shared channel is a legitimate — if unfair — arrangement of lanes:
/// every session dispatches into the same queue, so whoever drains that queue
/// decides the order of *everything*, not just of one connection's commands.
/// The daemon does not run this way; tests that want one queue they can read
/// commands off do, and this is what lets them pass a plain sender.
impl CommandLanes for mpsc::Sender<CommandDispatch> {
    fn open_lane(&self) -> mpsc::Sender<CommandDispatch> {
        self.clone()
    }
}

/// Shared ownership does not change how lanes are opened, so a lane source
/// behind an `Arc` is one too — which is what lets the daemon keep a handle on
/// its lanes (to wait for them at shutdown) while the accept loop uses them.
impl<T: CommandLanes + ?Sized> CommandLanes for Arc<T> {
    fn open_lane(&self) -> mpsc::Sender<CommandDispatch> {
        (**self).open_lane()
    }
}

/// Tracks which topics a single IPC client has subscribed to.
#[derive(Default)]
pub struct SessionSubscriptions(HashSet<Topic>);

impl SessionSubscriptions {
    pub fn contains(&self, topic: Topic) -> bool {
        self.0.contains(&topic)
    }

    pub fn insert(&mut self, topic: Topic) {
        self.0.insert(topic);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::ipc::protocol::{LogEvent, LogLevel, ObsEventPayload, Topic};

    use super::*;

    #[tokio::test]
    async fn publish_log_sends_typed_log_event_on_logs_topic() {
        let hub = BroadcastHub::new();
        let mut logs_rx = hub.subscribe_logs();

        hub.publish_log(LogEvent::new(LogLevel::Warn, "OBS unavailable"));

        let msg = logs_rx.recv().await.unwrap();
        match msg {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, Topic::Logs);
                let event: LogEvent = serde_json::from_value(data).unwrap();
                assert_eq!(event.level, LogLevel::Warn);
                assert_eq!(event.message, "OBS unavailable");
            }
            other => panic!("expected log event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_log_does_not_reach_non_log_subscribers() {
        let hub = BroadcastHub::new();
        let mut state_rx = hub.subscribe_state();
        let mut events_rx = hub.subscribe_events();

        hub.publish_log(LogEvent::new(LogLevel::Info, "daemon listening"));

        assert!(state_rx.try_recv().is_err());
        assert!(events_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn publish_obs_event_sends_typed_event_on_events_topic() {
        let hub = BroadcastHub::new();
        let mut events_rx = hub.subscribe_events();

        hub.publish_obs_event(ObsEventPayload::InputMuteStateChanged {
            input_name: "Mic".to_string(),
            muted: true,
        });

        let msg = events_rx.recv().await.unwrap();
        match msg {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, Topic::Events);
                let event: ObsEventPayload = serde_json::from_value(data.clone()).unwrap();
                assert_eq!(
                    event,
                    ObsEventPayload::InputMuteStateChanged {
                        input_name: "Mic".to_string(),
                        muted: true,
                    }
                );
                assert_eq!(
                    data,
                    json!({
                        "type": "InputMuteStateChanged",
                        "input_name": "Mic",
                        "muted": true
                    })
                );
            }
            other => panic!("expected OBS event, got {other:?}"),
        }
    }
}
