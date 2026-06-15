use std::collections::HashSet;
use tokio::sync::{broadcast, oneshot};

use crate::ipc::protocol::{CommandPayload, ServerMessage, TOPIC_EVENTS, TOPIC_LOGS, TOPIC_STATE};

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
        Self { state_tx, events_tx, logs_tx }
    }

    pub fn publish(&self, topic: &str, msg: ServerMessage) {
        let _ = match topic {
            TOPIC_STATE => self.state_tx.send(msg),
            TOPIC_EVENTS => self.events_tx.send(msg),
            TOPIC_LOGS => self.logs_tx.send(msg),
            _ => return,
        };
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
