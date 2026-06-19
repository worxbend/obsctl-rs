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
    pub fn new(code: PublicErrorCode, message: impl AsRef<str>) -> Self {
        Self::from_code(code.as_str(), message)
    }

    pub fn from_code(code: impl Into<String>, message: impl AsRef<str>) -> Self {
        Self {
            code: code.into(),
            message: redacted_message(message),
        }
    }
}

/// Canonical public error taxonomy for daemon-reachable IPC responses.
///
/// These strings are part of the public wire contract between the daemon and
/// CLI/TUI proxy clients. Keep this enum as the single audited source for IPC
/// error code strings and their proxy CLI exit-code mapping. Local process
/// failures that do not cross the daemon IPC boundary continue to use
/// `ObsctlError::exit_code()`.
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

    /// CLI exit code used when a proxy command receives this public IPC error.
    ///
    /// This mapping intentionally describes daemon-reachable failures. Local
    /// commands and startup failures use `ObsctlError::exit_code()` because
    /// their process context can classify failures differently before any IPC
    /// response exists.
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

    /// Convert an internal error into the public daemon IPC error taxonomy.
    ///
    /// This is the boundary where internal variants are collapsed into stable
    /// wire-visible classes. Preserve existing strings unless a compatibility
    /// test documents an intentional public contract change.
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
    let message = redact_url_credentials(message);
    let mut redacted = String::with_capacity(message.len());
    let mut scan_at = 0;
    let mut copy_from = 0;

    while scan_at < message.len() {
        if !message.is_char_boundary(scan_at) {
            scan_at += 1;
            continue;
        }

        let Some(value_range) = secret_value_range(&message, scan_at) else {
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
    redact_bearer_tokens(&redacted)
}

fn redact_url_credentials(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut cursor = 0;

    while let Some(scheme_offset) = message[cursor..].find("://") {
        let scheme_end = cursor + scheme_offset + 3;
        let host_end = url_authority_end(message, scheme_end);
        let Some(at_offset) = message[scheme_end..host_end].find('@') else {
            redacted.push_str(&message[cursor..host_end]);
            cursor = host_end;
            continue;
        };

        let at_index = scheme_end + at_offset;
        redacted.push_str(&message[cursor..scheme_end]);
        redacted.push_str(REDACTED_SECRET);
        redacted.push_str(&message[at_index..host_end]);
        cursor = host_end;
    }

    redacted.push_str(&message[cursor..]);
    redacted
}

fn url_authority_end(message: &str, authority_start: usize) -> usize {
    for (offset, ch) in message[authority_start..].char_indices() {
        if ch.is_whitespace() || matches!(ch, '/' | '?' | '#') {
            return authority_start + offset;
        }
    }
    message.len()
}

fn redact_bearer_tokens(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut scan_at = 0;
    let mut copy_from = 0;

    while scan_at < message.len() {
        if !message.is_char_boundary(scan_at) {
            scan_at += 1;
            continue;
        }

        let Some(value_range) = bearer_token_value_range(message, scan_at) else {
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

fn bearer_token_value_range(message: &str, bearer_start: usize) -> Option<std::ops::Range<usize>> {
    const BEARER: &str = "bearer";
    let bearer_end = bearer_start.checked_add(BEARER.len())?;
    if bearer_end > message.len() || !message[bearer_start..bearer_end].eq_ignore_ascii_case(BEARER)
    {
        return None;
    }
    if !is_key_boundary_before(message, bearer_start) {
        return None;
    }

    let mut cursor = bearer_end;
    if !matches!(message[cursor..].chars().next(), Some(ch) if ch.is_whitespace()) {
        return None;
    }
    cursor = consume_whitespace(message, cursor);
    if cursor >= message.len() {
        return None;
    }

    let value_end = redacted_literal_end(message, cursor)
        .unwrap_or_else(|| unquoted_value_end(message, cursor));
    if value_end == cursor {
        None
    } else {
        Some(cursor..value_end)
    }
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
            let value_end = redacted_literal_end(message, cursor)
                .unwrap_or_else(|| unquoted_value_end(message, cursor));
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

fn redacted_literal_end(message: &str, value_start: usize) -> Option<usize> {
    message[value_start..]
        .starts_with(REDACTED_SECRET)
        .then_some(value_start + REDACTED_SECRET.len())
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

    fn assert_wire_json<T>(message: &T, expected_raw: &str, expected_value: serde_json::Value)
    where
        T: Serialize,
    {
        let raw = serde_json::to_string(message).unwrap();

        assert_eq!(raw, expected_raw);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
            expected_value
        );
    }

    fn fixed_log_event() -> LogEvent {
        LogEvent {
            level: LogLevel::Info,
            message: "daemon listening".to_string(),
            target: Some("obsctl_rs::server".to_string()),
            timestamp: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn command_request_wire_json_is_stable() {
        let message = ClientMessage::Command {
            id: "req-000001".to_string(),
            command: CommandPayload {
                name: "set_scene".to_string(),
                args: json!({ "target": "main" }),
            },
        };

        assert_wire_json(
            &message,
            r#"{"type":"command","id":"req-000001","command":{"name":"set_scene","target":"main"}}"#,
            json!({
                "type": "command",
                "id": "req-000001",
                "command": {
                    "name": "set_scene",
                    "target": "main"
                }
            }),
        );
    }

    #[test]
    fn subscribe_request_wire_json_is_stable() {
        let message = ClientMessage::Subscribe {
            id: "req-000002".to_string(),
            topics: vec![
                TOPIC_STATE.to_string(),
                TOPIC_EVENTS.to_string(),
                TOPIC_LOGS.to_string(),
            ],
        };

        assert_wire_json(
            &message,
            r#"{"type":"subscribe","id":"req-000002","topics":["state","events","logs"]}"#,
            json!({
                "type": "subscribe",
                "id": "req-000002",
                "topics": ["state", "events", "logs"]
            }),
        );
    }

    #[test]
    fn success_response_wire_json_is_stable() {
        let message = ServerMessage::Response {
            id: "req-000001".to_string(),
            ok: true,
            result: Some(json!({ "message": "ok" })),
            error: None,
        };

        assert_wire_json(
            &message,
            r#"{"type":"response","id":"req-000001","ok":true,"result":{"message":"ok"}}"#,
            json!({
                "type": "response",
                "id": "req-000001",
                "ok": true,
                "result": {
                    "message": "ok"
                }
            }),
        );
    }

    #[test]
    fn error_response_wire_json_covers_all_public_error_codes() {
        assert_eq!(PublicErrorCode::ALL.len(), 12);

        for code in PublicErrorCode::ALL {
            let message = ServerMessage::Response {
                id: "req-error".to_string(),
                ok: false,
                result: None,
                error: Some(ErrorPayload::new(code, "representative error")),
            };
            let value = serde_json::to_value(&message).unwrap();

            assert_eq!(
                value,
                json!({
                    "type": "response",
                    "id": "req-error",
                    "ok": false,
                    "error": {
                        "code": code.as_str(),
                        "message": "representative error"
                    }
                })
            );
            assert_eq!(value["error"]["code"], code.as_str());
            assert!(PublicErrorCode::parse(value["error"]["code"].as_str().unwrap()).is_some());
        }
    }

    #[test]
    fn state_event_wire_json_is_stable() {
        let message = ServerMessage::Event {
            topic: TOPIC_STATE.to_string(),
            data: json!({
                "connected": true,
                "obs_studio_version": "30.1.2",
                "obs_websocket_version": "5.3.0",
                "current_scene": "Main",
                "scenes": [
                    {
                        "name": "Main",
                        "alias": "main",
                        "shortcut": "1",
                        "group": "live",
                        "active": true
                    }
                ],
                "audio_inputs": [
                    {
                        "name": "Mic",
                        "alias": "mic",
                        "shortcut": "m",
                        "kind": "wasapi_input_capture",
                        "muted": false,
                        "volume_mul": 0.75,
                        "volume_db": -2.5,
                        "volume_percent": 75
                    }
                ],
                "last_error": null,
                "updated_at": "2024-01-02T03:04:05Z"
            }),
        };
        let value = serde_json::to_value(&message).unwrap();

        assert_eq!(
            value,
            json!({
                "type": "event",
                "topic": "state",
                "data": {
                    "connected": true,
                    "obs_studio_version": "30.1.2",
                    "obs_websocket_version": "5.3.0",
                    "current_scene": "Main",
                    "scenes": [
                        {
                            "name": "Main",
                            "alias": "main",
                            "shortcut": "1",
                            "group": "live",
                            "active": true
                        }
                    ],
                    "audio_inputs": [
                        {
                            "name": "Mic",
                            "alias": "mic",
                            "shortcut": "m",
                            "kind": "wasapi_input_capture",
                            "muted": false,
                            "volume_mul": 0.75,
                            "volume_db": -2.5,
                            "volume_percent": 75
                        }
                    ],
                    "last_error": null,
                    "updated_at": "2024-01-02T03:04:05Z"
                }
            })
        );
        assert_eq!(value["topic"], TOPIC_STATE);
    }

    #[test]
    fn obs_event_wire_json_is_stable() {
        let message = ServerMessage::Event {
            topic: TOPIC_EVENTS.to_string(),
            data: json!({
                "type": "CurrentProgramSceneChanged",
                "scene_name": "BRB"
            }),
        };

        assert_wire_json(
            &message,
            r#"{"type":"event","topic":"events","data":{"scene_name":"BRB","type":"CurrentProgramSceneChanged"}}"#,
            json!({
                "type": "event",
                "topic": "events",
                "data": {
                    "type": "CurrentProgramSceneChanged",
                    "scene_name": "BRB"
                }
            }),
        );
    }

    #[test]
    fn log_event_wire_json_keeps_generic_event_envelope() {
        let message = ServerMessage::log_event(fixed_log_event());
        let value = serde_json::to_value(&message).unwrap();

        assert_eq!(value["type"], "event");
        assert_eq!(value["topic"], TOPIC_LOGS);
        assert_eq!(value["data"]["level"], "info");
        assert_eq!(value["data"]["message"], "daemon listening");
        assert_eq!(value["data"]["target"], "obsctl_rs::server");
        assert_eq!(value["data"]["timestamp"], "1970-01-01T00:00:00Z");

        assert_wire_json(
            &message,
            r#"{"type":"event","topic":"logs","data":{"level":"info","message":"daemon listening","target":"obsctl_rs::server","timestamp":"1970-01-01T00:00:00Z"}}"#,
            json!({
                "type": "event",
                "topic": "logs",
                "data": {
                    "level": "info",
                    "message": "daemon listening",
                    "target": "obsctl_rs::server",
                    "timestamp": "1970-01-01T00:00:00Z"
                }
            }),
        );
    }

    #[test]
    fn log_event_without_target_omits_optional_target_field() {
        let message = ServerMessage::log_event(LogEvent {
            level: LogLevel::Warn,
            message: "OBS unavailable".to_string(),
            target: None,
            timestamp: OffsetDateTime::UNIX_EPOCH,
        });

        assert_wire_json(
            &message,
            r#"{"type":"event","topic":"logs","data":{"level":"warn","message":"OBS unavailable","timestamp":"1970-01-01T00:00:00Z"}}"#,
            json!({
                "type": "event",
                "topic": "logs",
                "data": {
                    "level": "warn",
                    "message": "OBS unavailable",
                    "timestamp": "1970-01-01T00:00:00Z"
                }
            }),
        );
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
    fn error_payload_constructor_redacts_config_like_secret_values() {
        let payload = ErrorPayload::new(
            PublicErrorCode::ConfigInvalid,
            "config invalid: connection.password: hunter2; token = abc.def",
        );

        assert_eq!(payload.code, "CONFIG_INVALID");
        assert!(!payload.message.contains("hunter2"));
        assert!(!payload.message.contains("abc.def"));
        assert_eq!(
            payload.message,
            "config invalid: connection.password: [REDACTED]; token = [REDACTED]"
        );
    }

    #[test]
    fn error_payload_constructor_redacts_url_credentials() {
        let payload = ErrorPayload::new(
            PublicErrorCode::ObsUnavailable,
            "connect failed for ws://studio:hunter2@localhost:4455/obs",
        );

        assert!(!payload.message.contains("studio"));
        assert!(!payload.message.contains("hunter2"));
        assert_eq!(
            payload.message,
            "connect failed for ws://[REDACTED]@localhost:4455/obs"
        );
    }

    #[test]
    fn error_payload_constructor_redacts_bearer_tokens() {
        let payload = ErrorPayload::new(
            PublicErrorCode::ServerError,
            "upstream rejected Authorization: Bearer eyJ.secret.token",
        );

        assert!(!payload.message.contains("eyJ.secret.token"));
        assert_eq!(
            payload.message,
            "upstream rejected Authorization: Bearer [REDACTED]"
        );
    }

    #[test]
    fn error_payload_constructor_redacts_mixed_case_sensitive_keys() {
        let payload = ErrorPayload::new(
            PublicErrorCode::ServerError,
            "Password='hunter2' AUTH: abc123",
        );

        assert!(!payload.message.contains("hunter2"));
        assert!(!payload.message.contains("abc123"));
        assert_eq!(payload.message, "Password='[REDACTED]' AUTH: [REDACTED]");
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
        let expected_codes = [
            PublicErrorCode::ConfigInvalid,
            PublicErrorCode::ServerUnavailable,
            PublicErrorCode::ObsUnavailable,
            PublicErrorCode::RequestTimeout,
            PublicErrorCode::ObsRequestFailed,
            PublicErrorCode::SceneNotFound,
            PublicErrorCode::AudioInputNotFound,
            PublicErrorCode::AliasAmbiguous,
            PublicErrorCode::CommandParseError,
            PublicErrorCode::IpcProtocolError,
            PublicErrorCode::ShutdownDisabled,
            PublicErrorCode::ServerError,
        ];
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

        assert_eq!(PublicErrorCode::ALL, expected_codes);
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
        const OBSCTL_ERROR_VARIANT_COUNT: usize = 18;

        let cases = [
            (
                ObsctlError::ConfigNotFound("/tmp/missing.yml".to_string()),
                PublicErrorCode::ConfigInvalid,
            ),
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
            (
                ObsctlError::IpcConnectionFailed("connection refused".to_string()),
                PublicErrorCode::ServerUnavailable,
            ),
            (
                ObsctlError::IpcProtocolError("bad frame".to_string()),
                PublicErrorCode::IpcProtocolError,
            ),
            (
                ObsctlError::ConnectionFailed("connect failed".to_string()),
                PublicErrorCode::ObsUnavailable,
            ),
            (
                ObsctlError::AuthenticationFailed,
                PublicErrorCode::ObsUnavailable,
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
                ObsctlError::ShutdownDisabled,
                PublicErrorCode::ShutdownDisabled,
            ),
            (
                ObsctlError::DumpConfigFailed("write failed".to_string()),
                PublicErrorCode::ServerError,
            ),
            (
                ObsctlError::ServiceInstallFailed("systemctl failed".to_string()),
                PublicErrorCode::ServerError,
            ),
            (
                ObsctlError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "disk failed",
                )),
                PublicErrorCode::ServerError,
            ),
        ];

        assert_eq!(cases.len(), OBSCTL_ERROR_VARIANT_COUNT);

        for (error, expected) in cases {
            assert_eq!(public_error_code(&error), expected, "{error}");
        }
    }
}
