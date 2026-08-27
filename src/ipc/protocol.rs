use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use time::OffsetDateTime;

use crate::domain::errors::ObsctlError;
use crate::support::redaction::redact_message;
use crate::support::validation::validate_no_control_or_whitespace;

/// Maximum size of one framed IPC JSON line, including the trailing newline.
pub const MAX_IPC_LINE_BYTES: usize = 64 * 1024;
pub const MAX_IPC_REQUEST_ID_BYTES: usize = 64;
pub const MAX_COMMAND_NAME_BYTES: usize = 64;

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

impl CommandPayload {
    /// A command that takes no arguments.
    pub fn simple(command: ServerCommand) -> Self {
        Self {
            name: command.name().to_string(),
            args: Value::Null,
        }
    }

    /// A command that names one scene, profile, collection, or audio input.
    ///
    /// `target` is passed through as given; callers are responsible for
    /// trimming and validating it first (see `support::validation`).
    pub fn with_target(command: ServerCommand, target: &str) -> Self {
        Self {
            name: command.name().to_string(),
            args: serde_json::json!({ "target": target }),
        }
    }

    /// `set_volume`, which is the only command taking a second argument.
    pub fn set_volume(target: &str, percent: u8) -> Self {
        Self {
            name: ServerCommand::SetVolume.name().to_string(),
            args: serde_json::json!({ "target": target, "percent": percent }),
        }
    }

    /// `save_scene_profile`, the only command carrying a list.
    ///
    /// `hidden` names the scenes the profile hides, spelled as OBS spells
    /// them. The argument mechanism needed nothing new for this: `args` is an
    /// untyped JSON object flattened into the command, so an array value is
    /// already legal — only a constructor for it was missing.
    ///
    /// `rename_from` names the profile this save is replacing, when the caller
    /// opened an editor on an existing profile. It makes a rename **one**
    /// command: the daemon moves the entry in place instead of the caller
    /// having to save under the new name and then delete the old one, which
    /// was two file writes with a window in between where the profile existed
    /// twice, and which lost the active-profile pointer whenever the rename
    /// happened to be about the profile in effect. Left out for a profile
    /// being created, which is not replacing anything.
    pub fn save_scene_profile(name: &str, hidden: &[String], rename_from: Option<&str>) -> Self {
        let mut args = serde_json::json!({ "target": name, "hidden": hidden });
        if let Some(previous) = rename_from
            && let Some(object) = args.as_object_mut()
        {
            object.insert(
                RENAME_FROM.to_string(),
                serde_json::Value::String(previous.to_string()),
            );
        }
        Self {
            name: ServerCommand::SaveSceneProfile.name().to_string(),
            args,
        }
    }
}

/// The one optional argument in the whole command vocabulary. Named here so
/// the constructor above, the spec row below and the daemon's reader cannot
/// spell it three different ways.
pub const RENAME_FROM: &str = "rename_from";

/// The commands the daemon accepts.
///
/// This enum and [`COMMANDS`] are the canonical vocabulary of the IPC
/// contract. The daemon parses incoming names through it, and the CLI and TUI
/// build their payloads from it, so each command is spelled out in exactly one
/// place — a typo in a client is a compile error rather than a runtime
/// `unknown command`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServerCommand {
    Ping,
    GetServerStatus,
    GetObsStatus,
    GetSnapshot,
    SetScene,
    SetProfile,
    SetSceneCollection,
    Mute,
    Unmute,
    ToggleMute,
    SetVolume,
    ValidateConfig,
    ReloadConfig,
    ReconnectObs,
    ShutdownServer,
    DumpConfig,
    ToggleStream,
    ToggleRecord,
    SetSceneProfile,
    ClearSceneProfile,
    SaveSceneProfile,
    DeleteSceneProfile,
    ListSceneProfiles,
}

/// One row of the command vocabulary: the wire name, the argument keys the
/// payload must carry, and the ones it may carry. A payload is rejected for
/// any key outside the two lists, so between them they are the whole shape of
/// the command. Both empty means the command accepts no arguments at all.
pub struct CommandSpec {
    pub command: ServerCommand,
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub optional: &'static [&'static str],
}

const TARGET: &[&str] = &["target"];
const NONE: &[&str] = &[];
const RENAME: &[&str] = &[RENAME_FROM];

/// Every command, in the order they appear in `ServerCommand`. Adding a
/// command means adding a row here and a handler arm in the executor; the
/// exhaustiveness guard in this module's tests catches a forgotten row.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        command: ServerCommand::Ping,
        name: "ping",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::GetServerStatus,
        name: "get_server_status",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::GetObsStatus,
        name: "get_obs_status",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::GetSnapshot,
        name: "get_snapshot",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::SetScene,
        name: "set_scene",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::SetProfile,
        name: "set_profile",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::SetSceneCollection,
        name: "set_scene_collection",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::Mute,
        name: "mute",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::Unmute,
        name: "unmute",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ToggleMute,
        name: "toggle_mute",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::SetVolume,
        name: "set_volume",
        args: &["target", "percent"],
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ValidateConfig,
        name: "validate_config",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ReloadConfig,
        name: "reload_config",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ReconnectObs,
        name: "reconnect_obs",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ShutdownServer,
        name: "shutdown_server",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::DumpConfig,
        name: "dump_config",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ToggleStream,
        name: "toggle_stream",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ToggleRecord,
        name: "toggle_record",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::SetSceneProfile,
        name: "set_scene_profile",
        args: TARGET,
        optional: NONE,
    },
    // Switching a scene profile off is its own command rather than
    // `set_scene_profile` with an empty target: a blank target is rejected as a
    // malformed payload, and a reserved sentinel name ("none", "off") would
    // collide with a scene profile a user is entitled to create.
    CommandSpec {
        command: ServerCommand::ClearSceneProfile,
        name: "clear_scene_profile",
        args: NONE,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::SaveSceneProfile,
        name: "save_scene_profile",
        args: &["target", "hidden"],
        optional: RENAME,
    },
    CommandSpec {
        command: ServerCommand::DeleteSceneProfile,
        name: "delete_scene_profile",
        args: TARGET,
        optional: NONE,
    },
    CommandSpec {
        command: ServerCommand::ListSceneProfiles,
        name: "list_scene_profiles",
        args: NONE,
        optional: NONE,
    },
];

impl ServerCommand {
    fn spec(self) -> &'static CommandSpec {
        COMMANDS
            .iter()
            .find(|spec| spec.command == self)
            .expect("every ServerCommand variant has a row in COMMANDS")
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    /// The argument keys a payload for this command must carry.
    pub fn required_args(self) -> &'static [&'static str] {
        self.spec().args
    }

    /// The argument keys a payload for this command may carry but need not.
    /// Anything outside `required_args` and this list is rejected.
    pub fn optional_args(self) -> &'static [&'static str] {
        self.spec().optional
    }

    pub fn parse(name: &str) -> Option<Self> {
        COMMANDS
            .iter()
            .find(|spec| spec.name == name)
            .map(|spec| spec.command)
    }
}

impl std::fmt::Display for ServerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
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

/// Turn the strings a client sent in a subscribe request into the topics the
/// session should be signed up for.
///
/// The request stays string-typed on the wire because that is what the
/// protocol promised; this is where those strings stop being strings. Topics
/// repeated in one request collapse to a single subscription, and the order
/// the client asked for is kept so the acknowledgement echoes something
/// recognisable back.
pub fn normalize_subscribe_topics<I, S>(topics: I) -> Result<Vec<Topic>, SubscribeTopicsError>
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

    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    let mut unknown: Vec<String> = Vec::new();

    for topic in topics {
        match Topic::parse(&topic) {
            Some(parsed) => {
                if seen.insert(parsed) {
                    normalized.push(parsed);
                }
            }
            // Report each unrecognised spelling once. A linear scan is
            // enough: the list was already capped at MAX_SUBSCRIBE_TOPICS.
            None => {
                if !unknown.contains(&topic) {
                    unknown.push(topic);
                }
            }
        }
    }

    if !unknown.is_empty() {
        return Err(SubscribeTopicsError::UnknownTopics(unknown));
    }

    Ok(normalized)
}

/// Shared shape of the request-id and command-name checks. The error strings
/// cross the wire, so `what` must produce exactly the messages clients
/// already see ("<what> must not be empty", "<what> is too long", ...).
fn validate_bounded_token(value: &str, max_bytes: usize, what: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{what} must not be empty"));
    }
    if value.len() > max_bytes {
        return Err(format!("{what} is too long"));
    }
    validate_no_control_or_whitespace(value).map_err(|error| format!("{what} {error}"))
}

pub fn validate_ipc_request_id(id: &str) -> Result<(), String> {
    validate_bounded_token(id, MAX_IPC_REQUEST_ID_BYTES, "request id")
}

pub fn validate_command_name(name: &str) -> Result<(), String> {
    validate_bounded_token(name, MAX_COMMAND_NAME_BYTES, "command name")
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
    CurrentProfileChanged {
        profile_name: String,
    },
    ProfileListChanged,
    CurrentSceneCollectionChanged {
        scene_collection_name: String,
    },
    SceneCollectionListChanged,
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
            message: redact_message(message),
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
            message: redact_message(message),
        }
    }
}

/// One row of the public error taxonomy: the code, the string that goes on
/// the wire, and the exit status a proxy CLI command ends with after seeing
/// it. Keeping the three together means a new error cannot be given a wire
/// name without also being given an exit code.
pub struct ErrorCodeSpec {
    pub code: PublicErrorCode,
    pub wire: &'static str,
    pub exit_code: i32,
}

/// Every public error code, in the order they appear in `PublicErrorCode`.
///
/// The wire strings and exit codes here are the daemon-reachable public
/// contract documented in the README; changing a row changes what scripts
/// calling `obsctl` observe.
pub const ERROR_CODES: &[ErrorCodeSpec] = &[
    ErrorCodeSpec {
        code: PublicErrorCode::ConfigInvalid,
        wire: "CONFIG_INVALID",
        exit_code: 2,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::ServerUnavailable,
        wire: "SERVER_UNAVAILABLE",
        exit_code: 3,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::ObsUnavailable,
        wire: "OBS_UNAVAILABLE",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::RequestTimeout,
        wire: "REQUEST_TIMEOUT",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::ObsRequestFailed,
        wire: "OBS_REQUEST_FAILED",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::SceneNotFound,
        wire: "SCENE_NOT_FOUND",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::AudioInputNotFound,
        wire: "AUDIO_INPUT_NOT_FOUND",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::ProfileNotFound,
        wire: "PROFILE_NOT_FOUND",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::SceneCollectionNotFound,
        wire: "SCENE_COLLECTION_NOT_FOUND",
        exit_code: 4,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::AliasAmbiguous,
        wire: "ALIAS_AMBIGUOUS",
        exit_code: 1,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::CommandParseError,
        wire: "COMMAND_PARSE_ERROR",
        exit_code: 5,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::IpcTimeout,
        wire: "IPC_TIMEOUT",
        exit_code: 6,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::IpcProtocolError,
        wire: "IPC_PROTOCOL_ERROR",
        exit_code: 6,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::ShutdownDisabled,
        wire: "SHUTDOWN_DISABLED",
        exit_code: 1,
    },
    ErrorCodeSpec {
        code: PublicErrorCode::ServerError,
        wire: "SERVER_ERROR",
        exit_code: 1,
    },
];

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
    ProfileNotFound,
    SceneCollectionNotFound,
    AliasAmbiguous,
    CommandParseError,
    IpcTimeout,
    IpcProtocolError,
    ShutdownDisabled,
    ServerError,
}

impl PublicErrorCode {
    /// Every code, built from `ERROR_CODES` so the two can never disagree.
    /// `cli::client_commands` walks this to prove its exit-code mapping
    /// covers the whole taxonomy.
    pub const ALL: [Self; ERROR_CODES.len()] = {
        // A `const` loop rather than `iter().map()`: iterators are not
        // available in a constant, and the array has to exist at compile
        // time because callers match on its length.
        let mut all = [Self::ServerError; ERROR_CODES.len()];
        let mut index = 0;
        while index < ERROR_CODES.len() {
            all[index] = ERROR_CODES[index].code;
            index += 1;
        }
        all
    };

    fn spec(self) -> &'static ErrorCodeSpec {
        ERROR_CODES
            .iter()
            .find(|spec| spec.code == self)
            .expect("every PublicErrorCode variant has a row in ERROR_CODES")
    }

    pub fn as_str(self) -> &'static str {
        self.spec().wire
    }

    /// CLI exit code used when a proxy command receives this public IPC error.
    ///
    /// This mapping intentionally describes daemon-reachable failures. Local
    /// commands and startup failures use `ObsctlError::exit_code()` because
    /// their process context can classify failures differently before any IPC
    /// response exists.
    pub fn exit_code(self) -> i32 {
        self.spec().exit_code
    }

    pub fn parse(code: &str) -> Option<Self> {
        ERROR_CODES
            .iter()
            .find(|spec| spec.wire == code)
            .map(|spec| spec.code)
    }

    /// Convert an internal error into the public daemon IPC error taxonomy.
    ///
    /// This is the boundary where internal variants are collapsed into stable
    /// wire-visible classes. Preserve existing strings unless a compatibility
    /// test documents an intentional public contract change.
    ///
    /// Private on purpose: `public_error_code` is the single public entry.
    fn from_obsctl_error(error: &ObsctlError) -> Self {
        match error {
            ObsctlError::ConfigNotFound(_) | ObsctlError::ConfigInvalid(_) => Self::ConfigInvalid,
            ObsctlError::ServerUnavailable { .. } | ObsctlError::IpcConnectionFailed(_) => {
                Self::ServerUnavailable
            }
            ObsctlError::IpcProtocolError(_) => Self::IpcProtocolError,
            ObsctlError::IpcTimeout { .. } => Self::IpcTimeout,
            ObsctlError::ConnectionFailed(_)
            | ObsctlError::AuthenticationFailed
            | ObsctlError::ObsUnavailable => Self::ObsUnavailable,
            ObsctlError::RequestTimeout => Self::RequestTimeout,
            ObsctlError::ObsRequestFailed(_) => Self::ObsRequestFailed,
            ObsctlError::SceneNotFound(_) => Self::SceneNotFound,
            ObsctlError::AudioInputNotFound(_) => Self::AudioInputNotFound,
            ObsctlError::ProfileNotFound(_) => Self::ProfileNotFound,
            ObsctlError::SceneCollectionNotFound(_) => Self::SceneCollectionNotFound,
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

/// The wire spellings of the three topics. `Topic` is the type to reason
/// with; these constants exist so callers that hand a topic to a
/// string-typed API (the subscribe wire field, test fixtures) name the same
/// literal `Topic::as_str` returns instead of retyping it.
pub const TOPIC_STATE: &str = "state";
pub const TOPIC_EVENTS: &str = "events";
pub const TOPIC_LOGS: &str = "logs";

/// Typed topic discriminant for `ServerMessage::Event`. Serializes as the
/// lowercase wire string ("state", "events", "logs") to preserve wire compat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Topic {
    State,
    Events,
    Logs,
}

impl Topic {
    /// Every topic, in the order a client normally cares about them. Walking
    /// this is how callers avoid a fourth place that lists the three topics.
    pub const ALL: [Self; 3] = [Self::State, Self::Events, Self::Logs];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::State => TOPIC_STATE,
            Self::Events => TOPIC_EVENTS,
            Self::Logs => TOPIC_LOGS,
        }
    }

    /// Turn a wire string back into a topic. `None` means the string is not
    /// one of the three topics — including when it is empty or carries
    /// control characters, because the comparison is exact.
    pub fn parse(topic: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == topic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::state::{AudioState, ObsSnapshot, SceneProfileState, SceneState};
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
            ObsEventPayload::CurrentProfileChanged { .. } => {}
            ObsEventPayload::ProfileListChanged => {}
            ObsEventPayload::CurrentSceneCollectionChanged { .. } => {}
            ObsEventPayload::SceneCollectionListChanged => {}
        }
    }

    /// Compile-time guard: a new `ServerCommand` variant fails to compile here
    /// until it is added to the match, which is the prompt to add its row to
    /// `COMMANDS` too. `every_command_has_a_spec` then proves the row exists.
    #[allow(dead_code)]
    fn _server_command_variant_guard(c: ServerCommand) {
        match c {
            ServerCommand::Ping => {}
            ServerCommand::GetServerStatus => {}
            ServerCommand::GetObsStatus => {}
            ServerCommand::GetSnapshot => {}
            ServerCommand::SetScene => {}
            ServerCommand::SetProfile => {}
            ServerCommand::SetSceneCollection => {}
            ServerCommand::Mute => {}
            ServerCommand::Unmute => {}
            ServerCommand::ToggleMute => {}
            ServerCommand::SetVolume => {}
            ServerCommand::ValidateConfig => {}
            ServerCommand::ReloadConfig => {}
            ServerCommand::ReconnectObs => {}
            ServerCommand::ShutdownServer => {}
            ServerCommand::DumpConfig => {}
            ServerCommand::ToggleStream => {}
            ServerCommand::ToggleRecord => {}
            ServerCommand::SetSceneProfile => {}
            ServerCommand::ClearSceneProfile => {}
            ServerCommand::SaveSceneProfile => {}
            ServerCommand::DeleteSceneProfile => {}
            ServerCommand::ListSceneProfiles => {}
        }
    }

    #[test]
    fn every_command_has_a_spec_and_round_trips_through_its_name() {
        const SERVER_COMMAND_VARIANT_COUNT: usize = 23;
        assert_eq!(COMMANDS.len(), SERVER_COMMAND_VARIANT_COUNT);

        for spec in COMMANDS {
            // `name()` panics if a variant has no row, so this exercises the
            // lookup in both directions for every command.
            assert_eq!(spec.command.name(), spec.name);
            assert_eq!(ServerCommand::parse(spec.name), Some(spec.command));
            assert!(
                validate_command_name(spec.name).is_ok(),
                "{} is not a legal wire name",
                spec.name
            );
        }

        let names: HashSet<&str> = COMMANDS.iter().map(|spec| spec.name).collect();
        assert_eq!(names.len(), COMMANDS.len(), "duplicate command name");
    }

    #[test]
    fn command_payload_constructors_use_the_shared_vocabulary() {
        assert_eq!(
            CommandPayload::simple(ServerCommand::Ping).name,
            ServerCommand::Ping.name()
        );

        let payload = CommandPayload::with_target(ServerCommand::SetScene, "Main");
        assert_eq!(payload.name, "set_scene");
        assert_eq!(payload.args["target"], "Main");

        let payload = CommandPayload::set_volume("Mic", 42);
        assert_eq!(payload.name, "set_volume");
        assert_eq!(payload.args["target"], "Mic");
        assert_eq!(payload.args["percent"], 42);

        let payload = CommandPayload::save_scene_profile(
            "streaming",
            &["Utility BG".to_string(), "Overlay Src".to_string()],
            None,
        );
        assert_eq!(payload.name, "save_scene_profile");
        assert_eq!(payload.args["target"], "streaming");
        assert_eq!(payload.args["hidden"], json!(["Utility BG", "Overlay Src"]));
        assert_eq!(payload.args.get(RENAME_FROM), None);
    }

    /// The one command whose payload carries a list. Pinned verbatim because
    /// the array — its key, and the fact that it sits beside `target` in the
    /// flattened command object rather than under a nested `args` — is the
    /// part of the wire contract a client could otherwise guess wrong.
    #[test]
    fn save_scene_profile_request_wire_json_is_stable() {
        let message = ClientMessage::Command {
            id: "req-000007".to_string(),
            command: CommandPayload::save_scene_profile(
                "streaming",
                &["Utility BG".to_string(), "Overlay Src".to_string()],
                None,
            ),
        };

        assert_wire_json(
            &message,
            r#"{"type":"command","id":"req-000007","command":{"name":"save_scene_profile","hidden":["Utility BG","Overlay Src"],"target":"streaming"}}"#,
            json!({
                "type": "command",
                "id": "req-000007",
                "command": {
                    "name": "save_scene_profile",
                    "target": "streaming",
                    "hidden": ["Utility BG", "Overlay Src"]
                }
            }),
        );
    }

    /// An empty list is still a list: `save_scene_profile` declares `hidden`
    /// as a required argument, so "this profile hides nothing" travels as `[]`
    /// rather than as a missing key.
    #[test]
    fn save_scene_profile_sends_an_empty_hidden_list_as_an_empty_array() {
        let payload = CommandPayload::save_scene_profile("everything", &[], None);

        assert_eq!(payload.args["hidden"], json!([]));
    }

    /// The optional argument, which is what makes a rename one command. It is
    /// left out entirely rather than sent as `null` when there is nothing to
    /// rename, so a daemon that has never heard of it still accepts the
    /// payloads every other save produces.
    #[test]
    fn save_scene_profile_carries_the_renamed_from_name_only_when_renaming() {
        let payload = CommandPayload::save_scene_profile("night", &[], Some("streaming"));

        assert_eq!(payload.args[RENAME_FROM], "streaming");
        assert_eq!(payload.args["target"], "night");
    }

    #[test]
    fn parse_rejects_unknown_command_names() {
        assert_eq!(ServerCommand::parse("does-not-exist"), None);
        assert_eq!(ServerCommand::parse("set_scene "), None);
        assert_eq!(ServerCommand::parse(""), None);
    }

    #[allow(dead_code)]
    fn _obsctl_error_variant_guard(e: ObsctlError) {
        match e {
            ObsctlError::ConfigNotFound(_) => {}
            ObsctlError::ConfigInvalid(_) => {}
            ObsctlError::ServerUnavailable { .. } => {}
            ObsctlError::IpcConnectionFailed(_) => {}
            ObsctlError::IpcProtocolError(_) => {}
            ObsctlError::IpcTimeout { .. } => {}
            ObsctlError::ConnectionFailed(_) => {}
            ObsctlError::AuthenticationFailed => {}
            ObsctlError::ObsUnavailable => {}
            ObsctlError::RequestTimeout => {}
            ObsctlError::ObsRequestFailed(_) => {}
            ObsctlError::SceneNotFound(_) => {}
            ObsctlError::AudioInputNotFound(_) => {}
            ObsctlError::ProfileNotFound(_) => {}
            ObsctlError::SceneCollectionNotFound(_) => {}
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
        assert_eq!(PublicErrorCode::ALL.len(), 15);

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
            scene_profiles: vec![SceneProfileState {
                name: "streaming".to_string(),
                hidden: vec!["Utility BG".to_string()],
            }],
            active_scene_profile: Some("streaming".to_string()),
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
                    "profiles": [],
                    "current_profile": null,
                    "scene_collections": [],
                    "current_scene_collection": null,
                    "scene_profiles": [
                        {
                            "name": "streaming",
                            "hidden": ["Utility BG"]
                        }
                    ],
                    "active_scene_profile": "streaming",
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
            (
                ObsEventPayload::CurrentProfileChanged {
                    profile_name: "Streaming".to_string(),
                },
                json!({
                    "type": "CurrentProfileChanged",
                    "profile_name": "Streaming"
                }),
            ),
            (
                ObsEventPayload::ProfileListChanged,
                json!({ "type": "ProfileListChanged" }),
            ),
            (
                ObsEventPayload::CurrentSceneCollectionChanged {
                    scene_collection_name: "Podcast".to_string(),
                },
                json!({
                    "type": "CurrentSceneCollectionChanged",
                    "scene_collection_name": "Podcast"
                }),
            ),
            (
                ObsEventPayload::SceneCollectionListChanged,
                json!({ "type": "SceneCollectionListChanged" }),
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
        let redacted = redact_message(
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
        let redacted = redact_message("password_env=OBS_WEBSOCKET_PASSWORD auth mode disabled");

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

    // Compile-time exhaustiveness guard: a new `PublicErrorCode` variant makes
    // this match fail to compile, which is the reminder to add its row to
    // `ERROR_CODES` too. `every_public_error_code_has_a_spec` then proves the
    // row exists.
    #[allow(dead_code)]
    fn _public_error_code_variant_guard(c: PublicErrorCode) {
        match c {
            PublicErrorCode::ConfigInvalid => {}
            PublicErrorCode::ServerUnavailable => {}
            PublicErrorCode::ObsUnavailable => {}
            PublicErrorCode::RequestTimeout => {}
            PublicErrorCode::ObsRequestFailed => {}
            PublicErrorCode::SceneNotFound => {}
            PublicErrorCode::AudioInputNotFound => {}
            PublicErrorCode::ProfileNotFound => {}
            PublicErrorCode::SceneCollectionNotFound => {}
            PublicErrorCode::AliasAmbiguous => {}
            PublicErrorCode::CommandParseError => {}
            PublicErrorCode::IpcTimeout => {}
            PublicErrorCode::IpcProtocolError => {}
            PublicErrorCode::ShutdownDisabled => {}
            PublicErrorCode::ServerError => {}
        }
    }

    #[test]
    fn every_public_error_code_has_a_spec_and_documented_cli_exit_code() {
        const PUBLIC_ERROR_CODE_VARIANT_COUNT: usize = 15;
        assert_eq!(ERROR_CODES.len(), PUBLIC_ERROR_CODE_VARIANT_COUNT);
        assert_eq!(PublicErrorCode::ALL.len(), ERROR_CODES.len());

        for (index, spec) in ERROR_CODES.iter().enumerate() {
            // `as_str()` panics if a variant has no row, so this exercises the
            // lookup in both directions for every code.
            assert_eq!(spec.code.as_str(), spec.wire);
            assert_eq!(PublicErrorCode::parse(spec.wire), Some(spec.code));
            assert_eq!(spec.code.exit_code(), spec.exit_code, "{}", spec.wire);
            assert_eq!(
                exit_code_for_public_error_code(spec.wire),
                spec.exit_code,
                "{}",
                spec.wire
            );
            assert_eq!(PublicErrorCode::ALL[index], spec.code);
        }

        let wire_strings: HashSet<&str> = ERROR_CODES.iter().map(|spec| spec.wire).collect();
        assert_eq!(
            wire_strings.len(),
            ERROR_CODES.len(),
            "duplicate error code wire string"
        );

        // The documented exit codes, spelled out once so a silent edit to a
        // row in `ERROR_CODES` shows up as a failing expectation here.
        assert_eq!(PublicErrorCode::ConfigInvalid.exit_code(), 2);
        assert_eq!(PublicErrorCode::ServerUnavailable.exit_code(), 3);
        assert_eq!(PublicErrorCode::ObsUnavailable.exit_code(), 4);
        assert_eq!(PublicErrorCode::RequestTimeout.exit_code(), 4);
        assert_eq!(PublicErrorCode::ObsRequestFailed.exit_code(), 4);
        assert_eq!(PublicErrorCode::SceneNotFound.exit_code(), 4);
        assert_eq!(PublicErrorCode::AudioInputNotFound.exit_code(), 4);
        assert_eq!(PublicErrorCode::ProfileNotFound.exit_code(), 4);
        assert_eq!(PublicErrorCode::SceneCollectionNotFound.exit_code(), 4);
        assert_eq!(PublicErrorCode::AliasAmbiguous.exit_code(), 1);
        assert_eq!(PublicErrorCode::CommandParseError.exit_code(), 5);
        assert_eq!(PublicErrorCode::IpcTimeout.exit_code(), 6);
        assert_eq!(PublicErrorCode::IpcProtocolError.exit_code(), 6);
        assert_eq!(PublicErrorCode::ShutdownDisabled.exit_code(), 1);
        assert_eq!(PublicErrorCode::ServerError.exit_code(), 1);

        assert_eq!(exit_code_for_public_error_code("UNKNOWN_CODE"), 1);
    }

    #[test]
    fn obsctl_errors_map_to_public_ipc_error_codes() {
        const OBSCTL_ERROR_VARIANT_COUNT: usize = 21;

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
                ObsctlError::IpcTimeout { seconds: 30 },
                PublicErrorCode::IpcTimeout,
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
                ObsctlError::ProfileNotFound("Streaming".to_string()),
                PublicErrorCode::ProfileNotFound,
            ),
            (
                ObsctlError::SceneCollectionNotFound("Podcast".to_string()),
                PublicErrorCode::SceneCollectionNotFound,
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
    fn normalize_subscribe_topics_removes_duplicates_preserving_order() {
        let topics =
            normalize_subscribe_topics(vec!["events", "state", "events", "logs", "state"]).unwrap();

        assert_eq!(topics, vec![Topic::Events, Topic::State, Topic::Logs]);
    }

    #[test]
    fn topic_round_trips_through_its_wire_string() {
        for topic in Topic::ALL {
            assert_eq!(Topic::parse(topic.as_str()), Some(topic));
            assert_eq!(
                serde_json::to_value(topic).unwrap(),
                json!(topic.as_str()),
                "{topic:?} must serialize as its wire string"
            );
        }

        assert_eq!(
            Topic::ALL.map(Topic::as_str),
            [TOPIC_STATE, TOPIC_EVENTS, TOPIC_LOGS]
        );
        assert_eq!(Topic::parse("bad"), None);
        assert_eq!(Topic::parse(""), None);
        assert_eq!(Topic::parse("events\t"), None);
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
            Ok(vec![Topic::State, Topic::Events, Topic::Logs])
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
