use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

use crate::domain::errors::ObsctlError;

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

impl ErrorPayload {
    pub fn new(code: PublicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.as_str().to_string(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublicErrorCode {
    ConfigInvalid,
    ServerUnavailable,
    ObsUnavailable,
    RequestTimeout,
    ObsRequestFailed,
    SceneNotFound,
    AudioInputNotFound,
    AliasAmbiguous,
    CommandParseError,
    IpcProtocolError,
    ShutdownDisabled,
    ServerError,
}

impl PublicErrorCode {
    pub const ALL: [Self; 12] = [
        Self::ConfigInvalid,
        Self::ServerUnavailable,
        Self::ObsUnavailable,
        Self::RequestTimeout,
        Self::ObsRequestFailed,
        Self::SceneNotFound,
        Self::AudioInputNotFound,
        Self::AliasAmbiguous,
        Self::CommandParseError,
        Self::IpcProtocolError,
        Self::ShutdownDisabled,
        Self::ServerError,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigInvalid => "CONFIG_INVALID",
            Self::ServerUnavailable => "SERVER_UNAVAILABLE",
            Self::ObsUnavailable => "OBS_UNAVAILABLE",
            Self::RequestTimeout => "REQUEST_TIMEOUT",
            Self::ObsRequestFailed => "OBS_REQUEST_FAILED",
            Self::SceneNotFound => "SCENE_NOT_FOUND",
            Self::AudioInputNotFound => "AUDIO_INPUT_NOT_FOUND",
            Self::AliasAmbiguous => "ALIAS_AMBIGUOUS",
            Self::CommandParseError => "COMMAND_PARSE_ERROR",
            Self::IpcProtocolError => "IPC_PROTOCOL_ERROR",
            Self::ShutdownDisabled => "SHUTDOWN_DISABLED",
            Self::ServerError => "SERVER_ERROR",
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            Self::ConfigInvalid => 2,
            Self::ServerUnavailable => 3,
            Self::ObsUnavailable
            | Self::RequestTimeout
            | Self::ObsRequestFailed
            | Self::SceneNotFound
            | Self::AudioInputNotFound => 4,
            Self::CommandParseError => 5,
            Self::IpcProtocolError => 6,
            Self::AliasAmbiguous | Self::ShutdownDisabled | Self::ServerError => 1,
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        match code {
            "CONFIG_INVALID" => Some(Self::ConfigInvalid),
            "SERVER_UNAVAILABLE" => Some(Self::ServerUnavailable),
            "OBS_UNAVAILABLE" => Some(Self::ObsUnavailable),
            "REQUEST_TIMEOUT" => Some(Self::RequestTimeout),
            "OBS_REQUEST_FAILED" => Some(Self::ObsRequestFailed),
            "SCENE_NOT_FOUND" => Some(Self::SceneNotFound),
            "AUDIO_INPUT_NOT_FOUND" => Some(Self::AudioInputNotFound),
            "ALIAS_AMBIGUOUS" => Some(Self::AliasAmbiguous),
            "COMMAND_PARSE_ERROR" => Some(Self::CommandParseError),
            "IPC_PROTOCOL_ERROR" => Some(Self::IpcProtocolError),
            "SHUTDOWN_DISABLED" => Some(Self::ShutdownDisabled),
            "SERVER_ERROR" => Some(Self::ServerError),
            _ => None,
        }
    }

    pub fn from_obsctl_error(error: &ObsctlError) -> Self {
        match error {
            ObsctlError::ConfigNotFound(_) | ObsctlError::ConfigInvalid(_) => Self::ConfigInvalid,
            ObsctlError::ServerUnavailable { .. } | ObsctlError::IpcConnectionFailed(_) => {
                Self::ServerUnavailable
            }
            ObsctlError::IpcProtocolError(_) => Self::IpcProtocolError,
            ObsctlError::ConnectionFailed(_)
            | ObsctlError::AuthenticationFailed
            | ObsctlError::ObsUnavailable => Self::ObsUnavailable,
            ObsctlError::RequestTimeout => Self::RequestTimeout,
            ObsctlError::ObsRequestFailed(_) => Self::ObsRequestFailed,
            ObsctlError::SceneNotFound(_) => Self::SceneNotFound,
            ObsctlError::AudioInputNotFound(_) => Self::AudioInputNotFound,
            ObsctlError::AliasAmbiguous(_) => Self::AliasAmbiguous,
            ObsctlError::CommandParseError(_) => Self::CommandParseError,
            ObsctlError::ShutdownDisabled => Self::ShutdownDisabled,
            ObsctlError::DumpConfigFailed(_)
            | ObsctlError::ServiceInstallFailed(_)
            | ObsctlError::Io(_) => Self::ServerError,
        }
    }
}

impl std::fmt::Display for PublicErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn public_error_code(error: &ObsctlError) -> PublicErrorCode {
    PublicErrorCode::from_obsctl_error(error)
}

pub fn exit_code_for_public_error_code(code: &str) -> i32 {
    PublicErrorCode::parse(code)
        .map(PublicErrorCode::exit_code)
        .unwrap_or(1)
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

    #[test]
    fn public_error_codes_have_documented_cli_exit_codes() {
        let cases = [
            (PublicErrorCode::ConfigInvalid, "CONFIG_INVALID", 2),
            (PublicErrorCode::ServerUnavailable, "SERVER_UNAVAILABLE", 3),
            (PublicErrorCode::ObsUnavailable, "OBS_UNAVAILABLE", 4),
            (PublicErrorCode::RequestTimeout, "REQUEST_TIMEOUT", 4),
            (PublicErrorCode::ObsRequestFailed, "OBS_REQUEST_FAILED", 4),
            (PublicErrorCode::SceneNotFound, "SCENE_NOT_FOUND", 4),
            (
                PublicErrorCode::AudioInputNotFound,
                "AUDIO_INPUT_NOT_FOUND",
                4,
            ),
            (PublicErrorCode::AliasAmbiguous, "ALIAS_AMBIGUOUS", 1),
            (PublicErrorCode::CommandParseError, "COMMAND_PARSE_ERROR", 5),
            (PublicErrorCode::IpcProtocolError, "IPC_PROTOCOL_ERROR", 6),
            (PublicErrorCode::ShutdownDisabled, "SHUTDOWN_DISABLED", 1),
            (PublicErrorCode::ServerError, "SERVER_ERROR", 1),
        ];

        assert_eq!(cases.len(), PublicErrorCode::ALL.len());

        for (code, wire, exit_code) in cases {
            assert!(
                PublicErrorCode::ALL.contains(&code),
                "{code:?} missing from ALL"
            );
            assert_eq!(code.as_str(), wire);
            assert_eq!(PublicErrorCode::parse(wire), Some(code));
            assert_eq!(code.exit_code(), exit_code);
            assert_eq!(exit_code_for_public_error_code(wire), exit_code);
        }

        assert_eq!(exit_code_for_public_error_code("UNKNOWN_CODE"), 1);
    }

    #[test]
    fn obsctl_errors_map_to_public_ipc_error_codes() {
        let cases = [
            (
                ObsctlError::ConfigInvalid("bad".to_string()),
                PublicErrorCode::ConfigInvalid,
            ),
            (
                ObsctlError::ServerUnavailable {
                    socket_path: "/tmp/obsctl.sock".to_string(),
                    message: "connect failed".to_string(),
                },
                PublicErrorCode::ServerUnavailable,
            ),
            (ObsctlError::ObsUnavailable, PublicErrorCode::ObsUnavailable),
            (ObsctlError::RequestTimeout, PublicErrorCode::RequestTimeout),
            (
                ObsctlError::ObsRequestFailed("request failed".to_string()),
                PublicErrorCode::ObsRequestFailed,
            ),
            (
                ObsctlError::SceneNotFound("main".to_string()),
                PublicErrorCode::SceneNotFound,
            ),
            (
                ObsctlError::AudioInputNotFound("mic".to_string()),
                PublicErrorCode::AudioInputNotFound,
            ),
            (
                ObsctlError::AliasAmbiguous("cam".to_string()),
                PublicErrorCode::AliasAmbiguous,
            ),
            (
                ObsctlError::CommandParseError("bad command".to_string()),
                PublicErrorCode::CommandParseError,
            ),
            (
                ObsctlError::IpcProtocolError("bad frame".to_string()),
                PublicErrorCode::IpcProtocolError,
            ),
            (
                ObsctlError::ShutdownDisabled,
                PublicErrorCode::ShutdownDisabled,
            ),
            (
                ObsctlError::DumpConfigFailed("write failed".to_string()),
                PublicErrorCode::ServerError,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(public_error_code(&error), expected, "{error}");
        }
    }
}
