use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

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

impl ServerMessage {
    pub fn log_event(event: LogEvent) -> Self {
        Self::Event {
            topic: TOPIC_LOGS.to_string(),
            data: serde_json::to_value(event).expect("LogEvent serialization should not fail"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    pub level: LogLevel,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl LogEvent {
    pub fn new(level: LogLevel, message: impl AsRef<str>) -> Self {
        Self {
            level,
            message: redacted_message(message),
            target: None,
            timestamp: OffsetDateTime::now_utc(),
        }
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
}

pub fn redacted_message(message: impl AsRef<str>) -> String {
    redact_associated_secret_values(message.as_ref())
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

const REDACTED_SECRET: &str = "[REDACTED]";
const SECRET_KEYS: [&str; 4] = ["authentication", "password", "token", "auth"];

fn redact_associated_secret_values(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut scan_at = 0;
    let mut copy_from = 0;

    while scan_at < message.len() {
        if !message.is_char_boundary(scan_at) {
            scan_at += 1;
            continue;
        }

        let Some(value_range) = secret_value_range(message, scan_at) else {
            scan_at += message[scan_at..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            continue;
        };

        redacted.push_str(&message[copy_from..value_range.start]);
        redacted.push_str(REDACTED_SECRET);
        scan_at = value_range.end;
        copy_from = value_range.end;
    }

    redacted.push_str(&message[copy_from..]);
    redacted
}

fn secret_value_range(message: &str, key_start: usize) -> Option<std::ops::Range<usize>> {
    for key in SECRET_KEYS {
        let key_end = key_start.checked_add(key.len())?;
        if key_end > message.len() || !message[key_start..key_end].eq_ignore_ascii_case(key) {
            continue;
        }
        if !is_key_boundary_before(message, key_start) {
            continue;
        }
        if let Some(range) = associated_value_range(message, key_end) {
            return Some(range);
        }
    }
    None
}

fn associated_value_range(message: &str, key_end: usize) -> Option<std::ops::Range<usize>> {
    if is_identifier_char(message[key_end..].chars().next()) {
        return None;
    }

    let mut cursor = key_end;
    cursor = consume_optional_key_quote(message, cursor);
    cursor = consume_whitespace(message, cursor);

    if !matches!(message[cursor..].chars().next(), Some(':') | Some('=')) {
        return None;
    }

    cursor += 1;
    cursor = consume_whitespace(message, cursor);

    if cursor >= message.len() {
        return None;
    }

    match message[cursor..].chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let value_start = cursor + quote.len_utf8();
            let value_end = quoted_value_end(message, value_start, quote);
            Some(value_start..value_end)
        }
        Some(_) => {
            let value_end = unquoted_value_end(message, cursor);
            if value_end == cursor {
                None
            } else {
                Some(cursor..value_end)
            }
        }
        None => None,
    }
}

fn consume_optional_key_quote(message: &str, cursor: usize) -> usize {
    match message[cursor..].chars().next() {
        Some('"' | '\'') => cursor + 1,
        _ => cursor,
    }
}

fn consume_whitespace(message: &str, mut cursor: usize) -> usize {
    while let Some(ch) = message[cursor..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
    }
    cursor
}

fn quoted_value_end(message: &str, value_start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, ch) in message[value_start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return value_start + offset;
        }
    }
    message.len()
}

fn unquoted_value_end(message: &str, value_start: usize) -> usize {
    for (offset, ch) in message[value_start..].char_indices() {
        if ch.is_whitespace() || matches!(ch, ',' | ';' | '}' | ']') {
            return value_start + offset;
        }
    }
    message.len()
}

fn is_key_boundary_before(message: &str, key_start: usize) -> bool {
    if key_start == 0 {
        return true;
    }
    !is_identifier_char(message[..key_start].chars().next_back())
}

fn is_identifier_char(ch: Option<char>) -> bool {
    matches!(ch, Some(c) if c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixed_log_event() -> LogEvent {
        LogEvent {
            level: LogLevel::Info,
            message: "daemon listening".to_string(),
            target: Some("obsctl_rs::server".to_string()),
            timestamp: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn log_event_wire_json_keeps_generic_event_envelope() {
        let message = ServerMessage::log_event(fixed_log_event());
        let value = serde_json::to_value(message).unwrap();

        assert_eq!(value["type"], "event");
        assert_eq!(value["topic"], TOPIC_LOGS);
        assert_eq!(value["data"]["level"], "info");
        assert_eq!(value["data"]["message"], "daemon listening");
        assert_eq!(value["data"]["target"], "obsctl_rs::server");
        assert_eq!(value["data"]["timestamp"], "1970-01-01T00:00:00Z");
    }

    #[test]
    fn log_event_round_trips_through_server_message_serde() {
        let event = fixed_log_event();
        let encoded = serde_json::to_string(&ServerMessage::log_event(event.clone())).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&encoded).unwrap();

        match decoded {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, TOPIC_LOGS);
                let decoded_event: LogEvent = serde_json::from_value(data).unwrap();
                assert_eq!(decoded_event, event);
            }
            ServerMessage::Response { .. } => panic!("expected event message"),
        }
    }

    #[test]
    fn log_event_constructor_redacts_obvious_secret_values() {
        let event = LogEvent::new(
            LogLevel::Warn,
            "password=hunter2 authentication: \"abc123\" auth='bearer' token=shh",
        );

        assert!(!event.message.contains("hunter2"));
        assert!(!event.message.contains("abc123"));
        assert!(!event.message.contains("bearer"));
        assert!(!event.message.contains("shh"));
        assert_eq!(event.message.matches(REDACTED_SECRET).count(), 4);
    }

    #[test]
    fn redacted_message_masks_json_like_secret_values() {
        let redacted = redacted_message(
            r#"connect payload {"password":"hunter2","token":"abc.def","safe":"ok"}"#,
        );

        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("abc.def"));
        assert_eq!(
            redacted,
            r#"connect payload {"password":"[REDACTED]","token":"[REDACTED]","safe":"ok"}"#
        );
    }

    #[test]
    fn redacted_message_does_not_mask_unassociated_words() {
        let redacted = redacted_message("password_env=OBS_WEBSOCKET_PASSWORD auth mode disabled");

        assert_eq!(
            redacted,
            "password_env=OBS_WEBSOCKET_PASSWORD auth mode disabled"
        );
    }

    #[test]
    fn log_level_serializes_as_lowercase_strings() {
        assert_eq!(
            serde_json::to_value(LogLevel::Trace).unwrap(),
            json!("trace")
        );
        assert_eq!(
            serde_json::to_value(LogLevel::Debug).unwrap(),
            json!("debug")
        );
        assert_eq!(serde_json::to_value(LogLevel::Info).unwrap(), json!("info"));
        assert_eq!(serde_json::to_value(LogLevel::Warn).unwrap(), json!("warn"));
        assert_eq!(
            serde_json::to_value(LogLevel::Error).unwrap(),
            json!("error")
        );
    }
}
