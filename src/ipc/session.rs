use std::collections::HashSet;
use tokio::sync::{broadcast, oneshot};

use crate::ipc::protocol::{
    CommandPayload, LogEvent, ServerMessage, TOPIC_EVENTS, TOPIC_LOGS, TOPIC_STATE,
};

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

    pub fn publish(&self, topic: &str, msg: ServerMessage) {
        let _ = match topic {
            TOPIC_STATE => self.state_tx.send(msg),
            TOPIC_EVENTS => self.events_tx.send(msg),
            TOPIC_LOGS => self.logs_tx.send(msg),
            _ => return,
        };
    }

    pub fn publish_log(&self, event: LogEvent) {
        self.publish(TOPIC_LOGS, ServerMessage::log_event(event));
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<ServerMessage> {
        self.state_tx.subscribe()
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServerMessage> {
        self.events_tx.subscribe()
    }

    pub fn subscribe_logs(&self) -> broadcast::Receiver<ServerMessage> {
        self.logs_tx.subscribe()
    }
}

/// Dispatched from an IPC session to the server's command executor.
pub struct CommandDispatch {
    pub id: String,
    pub payload: CommandPayload,
    pub reply: oneshot::Sender<ServerMessage>,
}

/// Tracks which topics a single IPC client has subscribed to.
#[derive(Default)]
pub struct SessionSubscriptions(HashSet<String>);

impl SessionSubscriptions {
    pub fn contains(&self, topic: &str) -> bool {
        self.0.contains(topic)
    }

    pub fn insert(&mut self, topic: String) {
        self.0.insert(topic);
    }

    pub fn is_state_subscribed(&self) -> bool {
        self.0.contains(TOPIC_STATE)
    }
}

#[cfg(test)]
mod tests {
    use crate::ipc::protocol::{LogEvent, LogLevel, TOPIC_LOGS};

    use super::*;

    #[tokio::test]
    async fn publish_log_sends_typed_log_event_on_logs_topic() {
        let hub = BroadcastHub::new();
        let mut logs_rx = hub.subscribe_logs();

        hub.publish_log(LogEvent::new(LogLevel::Warn, "OBS unavailable"));

        let msg = logs_rx.recv().await.unwrap();
        match msg {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, TOPIC_LOGS);
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
}
