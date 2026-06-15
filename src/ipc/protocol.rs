use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Command { id: String, command: CommandPayload },
    Subscribe { id: String, topics: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandPayload {
    pub name: String,
    #[serde(flatten)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Response {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ErrorPayload>,
    },
    Event {
        topic: String,
        data: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

pub const TOPIC_STATE: &str = "state";
pub const TOPIC_EVENTS: &str = "events";
pub const TOPIC_LOGS: &str = "logs";

pub fn is_valid_topic(topic: &str) -> bool {
    matches!(topic, TOPIC_STATE | TOPIC_EVENTS | TOPIC_LOGS)
}
