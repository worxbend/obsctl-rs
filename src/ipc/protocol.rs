use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use time::OffsetDateTime;

use crate::domain::errors::ObsctlError;
pub use crate::support::redaction::redact_message as redacted_message;
use crate::support::validation::validate_no_control_or_whitespace;

/// Maximum size of one framed IPC JSON line, including the trailing newline.
pub const MAX_IPC_LINE_BYTES: usize = 64 * 1024;
pub const MAX_IPC_REQUEST_ID_BYTES: usize = 64;
pub const MAX_COMMAND_NAME_BYTES: usize = 64;

/// Client side tolerance for unrelated frames while waiting for a correlated response.
pub const MAX_UNMATCHED_IPC_RESPONSES: usize = 32;

/// Maximum number of topics accepted in a single subscribe command.
pub const MAX_SUBSCRIBE_TOPICS: usize = 16;

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
        topic: Topic,
        data: Value,
    },
}

impl ServerMessage {
    pub fn obs_event(event: ObsEventPayload) -> Self {
        Self::Event {
            topic: Topic::Events,
            data: serialize_payload(event, "ObsEventPayload"),
        }
    }

    pub fn log_event(event: LogEvent) -> Self {
        Self::Event {
            topic: Topic::Logs,
            data: serialize_payload(event, "LogEvent"),
        }
    }
}

fn serialize_payload<T: Serialize>(payload: T, label: &'static str) -> Value {
    serde_json::to_value(payload).unwrap_or_else(|error| {
        tracing::warn!(error = %error, label = %label, "failed to serialize IPC payload");
        serde_json::json!({
            "_serialization_error": true,
            "_payload": label,
            "_message": error.to_string(),
        })
    })
}

pub(crate) fn deduplicate_topics<I, S>(topics: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for topic in topics {
        let topic = topic.as_ref();
        if seen.insert(topic.to_string()) {
            deduped.push(topic.to_string());
        }
    }

    deduped
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeTopicsError {
    EmptyList,
    TooManyTopics { provided: usize, limit: usize },
    UnknownTopics(Vec<String>),
}

impl SubscribeTopicsError {
    pub fn as_protocol_error_message(&self) -> String {
        match self {
            Self::EmptyList => "topics list must not be empty".to_string(),
            Self::TooManyTopics { .. } => "too many topics in subscribe request".to_string(),
            Self::UnknownTopics(invalid) => {
                format!("unknown topics: {}", invalid.join(", "))
            }
        }
    }

    pub fn to_protocol_response(&self, id: String) -> ServerMessage {
        ServerMessage::Response {
            id,
            ok: false,
            result: None,
            error: Some(ErrorPayload::new(
                PublicErrorCode::IpcProtocolError,
                self.as_protocol_error_message(),
            )),
        }
    }
}

pub fn normalize_subscribe_topics<I, S>(topics: I) -> Result<Vec<String>, SubscribeTopicsError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let topics = topics
        .into_iter()
        .map(|topic| topic.as_ref().to_string())
        .collect::<Vec<_>>();

    if topics.is_empty() {
        return Err(SubscribeTopicsError::EmptyList);
    }

    if topics.len() > MAX_SUBSCRIBE_TOPICS {
        return Err(SubscribeTopicsError::TooManyTopics {
            provided: topics.len(),
            limit: MAX_SUBSCRIBE_TOPICS,
        });
    }

    let deduped = deduplicate_topics(topics);
    let invalid = deduped
        .iter()
        .filter(|topic| has_invalid_topic_chars(topic) || !is_valid_topic(topic))
        .cloned()
        .collect::<Vec<_>>();

    if !invalid.is_empty() {
        return Err(SubscribeTopicsError::UnknownTopics(invalid));
    }

    Ok(deduped)
}

fn has_invalid_topic_chars(topic: &str) -> bool {
    topic.is_empty() || validate_no_control_or_whitespace(topic).is_err()
}

pub fn validate_ipc_request_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("request id must not be empty".to_string());
    }
    if id.len() > MAX_IPC_REQUEST_ID_BYTES {
        return Err("request id is too long".to_string());
    }
    validate_no_control_or_whitespace(id).map_err(|error| format!("request id {error}"))?;
    Ok(())
}

pub fn validate_command_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("command name must not be empty".to_string());
    }
    if name.len() > MAX_COMMAND_NAME_BYTES {
        return Err("command name is too long".to_string());
    }
    validate_no_control_or_whitespace(name).map_err(|error| format!("command name {error}"))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputMeterLevel {
    pub name: String,
    pub level: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ObsEventPayload {
    CurrentProgramSceneChanged {
        scene_name: String,
    },
    SceneListChanged,
    InputCreated {
        input_name: String,
    },
    InputRemoved {
        input_name: String,
    },
    InputMuteStateChanged {
        input_name: String,
        muted: bool,
    },
    InputVolumeChanged {
        input_name: String,
        volume_mul: f64,
        volume_db: f64,
    },
    InputVolumeMeters {
        inputs: Vec<InputMeterLevel>,
    },
    StreamStateChanged {
        active: bool,
    },
    RecordStateChanged {
        active: bool,
    },
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

/// Typed topic discriminant for `ServerMessage::Event`. Serializes as the
/// lowercase wire string ("state", "events", "logs") to preserve wire compat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Topic {
    State,
    Events,
    Logs,
}

pub fn is_valid_topic(topic: &str) -> bool {
    matches!(topic, TOPIC_STATE | TOPIC_EVENTS | TOPIC_LOGS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::state::{AudioState, ObsSnapshot, SceneState};
    use crate::support::redaction::REDACTED_SECRET;
    use serde_json::json;
    use time::macros::datetime;

    // Compile-time exhaustiveness guards: fail to compile when a new variant is
    // added without updating this match, forcing the test cases to be updated too.
    #[allow(dead_code)]
    fn _obs_event_payload_variant_guard(p: ObsEventPayload) {
        match p {
            ObsEventPayload::CurrentProgramSceneChanged { .. } => {}
            ObsEventPayload::SceneListChanged => {}
            ObsEventPayload::InputCreated { .. } => {}
            ObsEventPayload::InputRemoved { .. } => {}
            ObsEventPayload::InputMuteStateChanged { .. } => {}
            ObsEventPayload::InputVolumeChanged { .. } => {}
            ObsEventPayload::InputVolumeMeters { .. } => {}
            ObsEventPayload::StreamStateChanged { .. } => {}
            ObsEventPayload::RecordStateChanged { .. } => {}
        }
    }

    #[allow(dead_code)]
    fn _obsctl_error_variant_guard(e: ObsctlError) {
        match e {
            ObsctlError::ConfigNotFound(_) => {}
            ObsctlError::ConfigInvalid(_) => {}
            ObsctlError::ServerUnavailable { .. } => {}
            ObsctlError::IpcConnectionFailed(_) => {}
            ObsctlError::IpcProtocolError(_) => {}
            ObsctlError::ConnectionFailed(_) => {}
            ObsctlError::AuthenticationFailed => {}
            ObsctlError::ObsUnavailable => {}
            ObsctlError::RequestTimeout => {}
            ObsctlError::ObsRequestFailed(_) => {}
            ObsctlError::SceneNotFound(_) => {}
            ObsctlError::AudioInputNotFound(_) => {}
            ObsctlError::AliasAmbiguous(_) => {}
            ObsctlError::CommandParseError(_) => {}
            ObsctlError::ShutdownDisabled => {}
            ObsctlError::DumpConfigFailed(_) => {}
            ObsctlError::ServiceInstallFailed(_) => {}
            ObsctlError::Io(_) => {}
        }
    }

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
        let snapshot = ObsSnapshot {
            connected: true,
            obs_studio_version: Some("30.1.2".to_string()),
            obs_websocket_version: Some("5.3.0".to_string()),
            current_scene: Some("Main".to_string()),
            scenes: vec![SceneState {
                name: "Main".to_string(),
                alias: Some("main".to_string()),
                shortcut: Some("1".to_string()),
                group: Some("live".to_string()),
                active: true,
                hidden: false,
            }],
            audio_inputs: vec![AudioState {
                name: "Mic".to_string(),
                alias: Some("mic".to_string()),
                shortcut: Some("m".to_string()),
                kind: Some("wasapi_input_capture".to_string()),
                muted: Some(false),
                volume_mul: Some(0.75),
                volume_db: Some(-2.5),
                volume_percent: Some(75),
            }],
            streaming: false,
            recording: false,
            updated_at: datetime!(2024-01-02 03:04:05 UTC),
            ..ObsSnapshot::default()
        };
        let message = ServerMessage::Event {
            topic: Topic::State,
            data: serde_json::to_value(snapshot).unwrap(),
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
                            "active": true,
                            "hidden": false
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
                    "streaming": false,
                    "recording": false,
                    "last_error": null,
                    "stats": null,
                    "stream_bitrate_kbps": null,
                    "stream_duration_ms": null,
                    "record_duration_ms": null,
                    "updated_at": "2024-01-02T03:04:05Z"
                }
            })
        );
        assert_eq!(value["topic"], TOPIC_STATE);
    }

    #[test]
    fn obs_event_wire_json_covers_public_payload_variants() {
        let cases = [
            (
                ObsEventPayload::CurrentProgramSceneChanged {
                    scene_name: "BRB".to_string(),
                },
                json!({
                    "type": "CurrentProgramSceneChanged",
                    "scene_name": "BRB"
                }),
            ),
            (
                ObsEventPayload::SceneListChanged,
                json!({
                    "type": "SceneListChanged"
                }),
            ),
            (
                ObsEventPayload::InputCreated {
                    input_name: "Mic".to_string(),
                },
                json!({
                    "type": "InputCreated",
                    "input_name": "Mic"
                }),
            ),
            (
                ObsEventPayload::InputRemoved {
                    input_name: "Mic".to_string(),
                },
                json!({
                    "type": "InputRemoved",
                    "input_name": "Mic"
                }),
            ),
            (
                ObsEventPayload::InputMuteStateChanged {
                    input_name: "Mic".to_string(),
                    muted: true,
                },
                json!({
                    "type": "InputMuteStateChanged",
                    "input_name": "Mic",
                    "muted": true
                }),
            ),
            (
                ObsEventPayload::InputVolumeChanged {
                    input_name: "Desktop Audio".to_string(),
                    volume_mul: 0.75,
                    volume_db: -2.5,
                },
                json!({
                    "type": "InputVolumeChanged",
                    "input_name": "Desktop Audio",
                    "volume_mul": 0.75,
                    "volume_db": -2.5
                }),
            ),
            (
                ObsEventPayload::InputVolumeMeters {
                    inputs: vec![InputMeterLevel {
                        name: "Mic".to_string(),
                        level: 0.4,
                    }],
                },
                json!({
                    "type": "InputVolumeMeters",
                    "inputs": [{ "name": "Mic", "level": 0.4f32 }]
                }),
            ),
            (
                ObsEventPayload::StreamStateChanged { active: true },
                json!({ "type": "StreamStateChanged", "active": true }),
            ),
            (
                ObsEventPayload::RecordStateChanged { active: false },
                json!({ "type": "RecordStateChanged", "active": false }),
            ),
        ];

        for (payload, expected_data) in cases {
            let message = ServerMessage::obs_event(payload);
            let value = serde_json::to_value(&message).unwrap();

            assert_eq!(
                value,
                json!({
                    "type": "event",
                    "topic": "events",
                    "data": expected_data
                })
            );
        }
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
                assert_eq!(topic, Topic::Logs);
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

    #[test]
    fn deduplicate_topics_removes_duplicates_preserving_order() {
        let topics = deduplicate_topics(vec!["events", "state", "events", "logs", "state"]);

        assert_eq!(topics, vec!["events", "state", "logs"]);
    }

    #[test]
    fn normalize_subscribe_topics_validates_limit_and_topic_names() {
        assert_eq!(
            normalize_subscribe_topics::<Vec<&str>, &str>(vec![]),
            Err(SubscribeTopicsError::EmptyList)
        );

        let too_many = normalize_subscribe_topics((0..=MAX_SUBSCRIBE_TOPICS).map(|idx| {
            if idx % 2 == 0 {
                TOPIC_STATE
            } else {
                TOPIC_EVENTS
            }
        }));
        assert!(matches!(
            too_many,
            Err(SubscribeTopicsError::TooManyTopics {
                provided,
                limit,
            }) if provided == MAX_SUBSCRIBE_TOPICS + 1 && limit == MAX_SUBSCRIBE_TOPICS
        ));

        assert_eq!(
            normalize_subscribe_topics(["state", "events", "bad"]),
            Err(SubscribeTopicsError::UnknownTopics(vec!["bad".to_string()]))
        );

        assert_eq!(
            normalize_subscribe_topics(["state", "", "events"]),
            Err(SubscribeTopicsError::UnknownTopics(vec!["".to_string()]))
        );

        assert_eq!(
            normalize_subscribe_topics(["state", "events\t"]),
            Err(SubscribeTopicsError::UnknownTopics(vec![
                "events\t".to_string()
            ]))
        );

        assert_eq!(
            normalize_subscribe_topics(["state", "state", TOPIC_EVENTS, TOPIC_LOGS]),
            Ok(vec![
                TOPIC_STATE.to_string(),
                TOPIC_EVENTS.to_string(),
                TOPIC_LOGS.to_string()
            ])
        );
    }

    #[test]
    fn subscribe_topics_error_builds_protocol_response() {
        let err = SubscribeTopicsError::UnknownTopics(vec!["bad-topic".to_string()]);
        let response = err.to_protocol_response("req-1".to_string());

        match response {
            ServerMessage::Response {
                id,
                ok,
                result,
                error,
            } => {
                assert_eq!(id, "req-1");
                assert!(!ok);
                assert!(result.is_none());
                let error = error.expect("missing protocol error payload");
                assert_eq!(error.code, PublicErrorCode::IpcProtocolError.as_str());
                assert_eq!(error.message, "unknown topics: bad-topic");
            }
            _ => panic!("expected response message"),
        }
    }

    #[test]
    fn validate_ipc_request_id_rejects_bad_values() {
        assert!(validate_ipc_request_id("ok-id").is_ok());
        assert!(validate_ipc_request_id("id with space").is_err());
        assert!(validate_ipc_request_id("").is_err());
        assert!(validate_ipc_request_id(&"x".repeat(MAX_IPC_REQUEST_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn validate_command_name_rejects_bad_values() {
        assert!(validate_command_name("set_scene").is_ok());
        assert!(validate_command_name("name with space").is_err());
        assert!(validate_command_name("").is_err());
        assert!(validate_command_name(&"x".repeat(MAX_COMMAND_NAME_BYTES + 1)).is_err());
    }
}
