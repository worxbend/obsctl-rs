use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::domain::{errors::ObsctlError, result::Result};
use crate::obs::protocol::{
    OPCODE_EVENT, OPCODE_IDENTIFY, OPCODE_REQUEST, OPCODE_REQUEST_RESPONSE, ObsMessage,
    RequestData, RequestResponseData,
};
use crate::obs::validation::extract_resource_names;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

const ES_GENERAL: u32 = 1;
/// Profile and scene-collection change/list events live under this scope in
/// obs-websocket, not `General` — without it, OBS never sends
/// `CurrentProfileChanged`/`ProfileListChanged`/`CurrentSceneCollectionChanged`/
/// `SceneCollectionListChanged` to this client.
const ES_CONFIG: u32 = 2;
const ES_SCENES: u32 = 4;
const ES_INPUTS: u32 = 8;
const ES_OUTPUTS: u32 = 64;
const ES_INPUT_VOLUME_METERS: u32 = 65536;
const EVENT_SUBSCRIPTIONS: u32 =
    ES_GENERAL | ES_CONFIG | ES_SCENES | ES_INPUTS | ES_OUTPUTS | ES_INPUT_VOLUME_METERS;

/// Events emitted by the OBS client to its supervisor.
#[derive(Debug, Clone, PartialEq)]
pub enum ObsEvent {
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
    /// High-frequency (~60 fps) per-input RMS magnitude, linear 0-1 scale.
    InputVolumeMeters {
        inputs: Vec<(String, f32)>,
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
    Other {
        event_type: String,
        data: Value,
    },
}

struct ObsClientRequest {
    request: RequestData,
    reply: oneshot::Sender<Result<Value>>,
}

/// Handle to a connected and identified OBS WebSocket session.
/// Cheap to clone; all clones share the same underlying connection.
#[derive(Clone, Debug)]
pub struct ObsClient {
    sender: mpsc::Sender<ObsClientRequest>,
    /// Channel to signal the client task to remove a timed-out pending entry.
    cancel_tx: mpsc::Sender<String>,
    request_timeout_ms: u64,
}

impl ObsClient {
    /// Send a request and wait for the response data.
    /// Returns `ObsctlError::RequestTimeout` if no response arrives within the configured timeout.
    pub async fn request(&self, req: RequestData) -> Result<Value> {
        let request_id = req.request_id.clone();
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(ObsClientRequest {
                request: req,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ObsctlError::ObsUnavailable)?;

        match tokio::time::timeout(Duration::from_millis(self.request_timeout_ms), reply_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ObsctlError::ObsUnavailable),
            Err(_timeout) => {
                // Notify the client task to remove the stale pending entry.
                let _ = self.cancel_tx.send(request_id).await;
                Err(ObsctlError::RequestTimeout)
            }
        }
    }
}

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Complete the WebSocket handshake, spawning the client task.
/// Returns `(client, obs_studio_version, obs_websocket_version, disconnect_rx)`.
/// `disconnect_rx` resolves when the underlying WebSocket task exits (OBS disconnect).
pub async fn handshake(
    mut sink: WsSink,
    mut stream: WsStream,
    password: Option<&str>,
    event_tx: mpsc::Sender<ObsEvent>,
    request_timeout_ms: u64,
) -> Result<(
    ObsClient,
    String,
    String,
    tokio::sync::oneshot::Receiver<()>,
)> {
    use crate::obs::protocol::{HelloData, IdentifyData, OPCODE_HELLO, OPCODE_IDENTIFIED};

    // 1. Read Hello
    let hello_raw = read_ws_message(&mut stream).await?;
    if hello_raw.op != OPCODE_HELLO {
        return Err(ObsctlError::ConnectionFailed(format!(
            "expected Hello (opcode {}), got {}",
            OPCODE_HELLO, hello_raw.op
        )));
    }
    let hello: HelloData = serde_json::from_value(hello_raw.d)
        .map_err(|e| ObsctlError::ConnectionFailed(format!("invalid Hello: {e}")))?;
    let obs_ws_version = hello.obs_web_socket_version.clone();

    // 2. Build Identify
    let authentication = if let Some(auth_cfg) = &hello.authentication {
        let pw = password.ok_or(ObsctlError::AuthenticationFailed)?;
        Some(crate::obs::auth::compute_authentication(
            pw,
            &auth_cfg.salt,
            &auth_cfg.challenge,
        ))
    } else {
        None
    };

    let identify = ObsMessage {
        op: OPCODE_IDENTIFY,
        d: serde_json::to_value(IdentifyData {
            rpc_version: 1,
            authentication,
            event_subscriptions: Some(EVENT_SUBSCRIPTIONS),
        })
        .map_err(|e| ObsctlError::ConnectionFailed(format!("serialize Identify: {e}")))?,
    };

    let identify_json = serde_json::to_string(&identify)
        .map_err(|e| ObsctlError::ConnectionFailed(format!("serialize: {e}")))?;
    sink.send(Message::Text(identify_json.clone()))
        .await
        .map_err(|e| ObsctlError::ConnectionFailed(e.to_string()))?;

    // 3. Wait for Identified
    let identified_raw = read_ws_message(&mut stream).await?;
    if identified_raw.op != OPCODE_IDENTIFIED {
        return Err(ObsctlError::AuthenticationFailed);
    }

    // Spawn the client task with a cancel channel for timeout cleanup.
    // The disconnect_tx is dropped when the task exits, signalling disconnect_rx.
    let (req_tx, req_rx) = mpsc::channel::<ObsClientRequest>(64);
    let (cancel_tx, cancel_rx) = mpsc::channel::<String>(64);
    let (disconnect_tx, disconnect_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(run_client_task(
        sink,
        stream,
        req_rx,
        cancel_rx,
        event_tx,
        disconnect_tx,
    ));

    let client = ObsClient {
        sender: req_tx,
        cancel_tx,
        request_timeout_ms,
    };

    // Fetch OBS version to return metadata
    let version_data = client.request(crate::obs::requests::get_version()).await?;
    let obs_studio_version = version_data
        .get("obsVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok((client, obs_studio_version, obs_ws_version, disconnect_rx))
}

pub(crate) async fn read_ws_message(stream: &mut WsStream) -> Result<ObsMessage> {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                return serde_json::from_str::<ObsMessage>(&text)
                    .map_err(|e| ObsctlError::ConnectionFailed(format!("parse obs message: {e}")));
            }
            Ok(Message::Binary(bin)) => {
                return serde_json::from_slice::<ObsMessage>(&bin)
                    .map_err(|e| ObsctlError::ConnectionFailed(format!("parse obs message: {e}")));
            }
            Ok(Message::Close(_)) => {
                return Err(ObsctlError::ConnectionFailed(
                    "WebSocket closed during handshake".to_string(),
                ));
            }
            Ok(_) => {}
            Err(e) => return Err(ObsctlError::ConnectionFailed(e.to_string())),
        }
    }
    Err(ObsctlError::ConnectionFailed(
        "WebSocket stream ended".to_string(),
    ))
}

async fn run_client_task(
    mut sink: WsSink,
    mut stream: WsStream,
    mut req_rx: mpsc::Receiver<ObsClientRequest>,
    mut cancel_rx: mpsc::Receiver<String>,
    event_tx: mpsc::Sender<ObsEvent>,
    _disconnect_tx: tokio::sync::oneshot::Sender<()>,
) {
    let mut pending: HashMap<String, oneshot::Sender<Result<Value>>> = HashMap::new();

    loop {
        tokio::select! {
            maybe_req = req_rx.recv() => {
                let Some(ObsClientRequest { request, reply }) = maybe_req else { break; };
                let id = request.request_id.clone();

                let d = match serde_json::to_value(&request) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = reply.send(Err(ObsctlError::ObsRequestFailed(e.to_string())));
                        continue;
                    }
                };
                let text = match serde_json::to_string(&ObsMessage { op: OPCODE_REQUEST, d }) {
                    Ok(t) => t,
                    Err(e) => {
                        let _ = reply.send(Err(ObsctlError::ObsRequestFailed(e.to_string())));
                        continue;
                    }
                };

                if sink.send(Message::Text(text)).await.is_err() {
                    let _ = reply.send(Err(ObsctlError::ConnectionFailed("WebSocket send failed".to_string())));
                    for (_, s) in pending.drain() {
                        let _ = s.send(Err(ObsctlError::ConnectionFailed("WebSocket closed".to_string())));
                    }
                    break;
                }
                pending.insert(id, reply);
            }

            maybe_cancel = cancel_rx.recv() => {
                if let Some(id) = maybe_cancel {
                    pending.remove(&id);
                }
            }

            maybe_msg = stream.next() => {
                match maybe_msg {
                    None => {
                        for (_, s) in pending.drain() {
                            let _ = s.send(Err(ObsctlError::ConnectionFailed("WebSocket closed".to_string())));
                        }
                        break;
                    }
                    Some(Err(e)) => {
                        for (_, s) in pending.drain() {
                            let _ = s.send(Err(ObsctlError::ConnectionFailed(e.to_string())));
                        }
                        break;
                    }
                    Some(Ok(msg)) => {
                        dispatch_message(msg, &mut pending, &event_tx).await;
                    }
                }
            }
        }
    }
}

async fn dispatch_message(
    msg: Message,
    pending: &mut HashMap<String, oneshot::Sender<Result<Value>>>,
    event_tx: &mpsc::Sender<ObsEvent>,
) {
    let text = match msg {
        Message::Text(t) => t,
        Message::Binary(b) => match String::from_utf8(b) {
            Ok(s) => s,
            Err(_) => return,
        },
        _ => return,
    };

    let obs_msg: ObsMessage = match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            warn!("Failed to parse OBS message: {e}");
            return;
        }
    };

    match obs_msg.op {
        OPCODE_REQUEST_RESPONSE => {
            let resp: RequestResponseData = match serde_json::from_value(obs_msg.d) {
                Ok(r) => r,
                Err(e) => {
                    warn!("Failed to parse RequestResponse: {e}");
                    return;
                }
            };
            if let Some(reply) = pending.remove(&resp.request_id) {
                if resp.request_status.result {
                    let _ = reply.send(Ok(resp.response_data.unwrap_or(Value::Null)));
                } else {
                    let comment = resp
                        .request_status
                        .comment
                        .unwrap_or_else(|| format!("code {}", resp.request_status.code));
                    let _ = reply.send(Err(ObsctlError::ObsRequestFailed(comment)));
                }
            }
        }
        OPCODE_EVENT => {
            dispatch_event(obs_msg.d, event_tx).await;
        }
        op => {
            debug!("Unhandled OBS opcode: {op}");
        }
    }
}

/// One OBS event payload, with the field lookups that every arm of the event
/// switch needs.
///
/// OBS sends `{"eventType": "...", "eventData": {...}}`. Every field read has
/// the same shape: pull the field, check its type (and, for strings, that it is
/// a safe token), and log exactly which field of which event was malformed
/// before giving up. Keeping that in one place is what lets the switch below
/// read as a list of event shapes instead of a wall of error handling.
struct EventPayload {
    event_type: String,
    data: Value,
}

impl EventPayload {
    /// Returns `None` — after logging — when the message has no usable
    /// `eventType`, which is the only field we cannot recover from.
    fn parse(message: Value) -> Option<Self> {
        let raw_type = message
            .get("eventType")
            .and_then(|v| v.as_str())
            .or_else(|| {
                warn!("Malformed OBS event: missing or invalid eventType");
                None
            })?;
        let event_type = trim_and_validate_token_with_max_len(raw_type, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| {
                warn!(
                    event_type = %raw_type,
                    message = %error,
                    "Malformed OBS event payload: invalid eventType"
                );
            })
            .ok()?;
        let data = message.get("eventData").cloned().unwrap_or(Value::Null);
        Some(Self { event_type, data })
    }

    fn warn_field(&self, field: &str, message: &str) {
        warn!(
            event_type = %self.event_type,
            field = %field,
            "{message}"
        );
    }

    fn required_str(&self, field: &str) -> Option<String> {
        let raw = self.data.get(field).and_then(|v| v.as_str()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid string field",
            );
            None
        })?;
        trim_and_validate_token_with_max_len(raw, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| {
                warn!(
                    event_type = %self.event_type,
                    field = %field,
                    message = %error,
                    "Malformed OBS event payload: string field invalid"
                );
            })
            .ok()
    }

    fn required_bool(&self, field: &str) -> Option<bool> {
        self.data.get(field).and_then(|v| v.as_bool()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid boolean field",
            );
            None
        })
    }

    fn required_f64(&self, field: &str) -> Option<f64> {
        self.data.get(field).and_then(|v| v.as_f64()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid number field",
            );
            None
        })
    }

    fn required_array(&self, field: &str) -> Option<&Vec<Value>> {
        self.data.get(field).and_then(|v| v.as_array()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid array field",
            );
            None
        })
    }

    /// A decibel level: any finite number, since audio dB is normally negative.
    ///
    /// The finiteness check is defence in depth rather than input validation —
    /// `serde_json` already rejects out-of-range numbers while parsing, so a
    /// NaN or infinity can only reach here from a payload built in-process.
    fn required_decibels(&self, field: &str) -> Option<f64> {
        let value = self.required_f64(field)?;
        if !value.is_finite() {
            self.warn_field(field, "Malformed OBS event payload: level is not finite");
            return None;
        }
        Some(value)
    }

    /// A linear volume multiplier: finite and at or above zero, because a
    /// negative multiplier would invert the waveform rather than attenuate it.
    fn required_volume_multiplier(&self, field: &str) -> Option<f64> {
        let value = self.required_decibels(field)?;
        if value < 0.0 {
            self.warn_field(
                field,
                "Malformed OBS event payload: volume multiplier must not be negative",
            );
            return None;
        }
        Some(value)
    }
}

async fn dispatch_event(data: Value, event_tx: &mpsc::Sender<ObsEvent>) {
    let Some(payload) = EventPayload::parse(data) else {
        return;
    };
    let Some(event) = translate_event(payload) else {
        return;
    };
    let _ = event_tx.send(event).await;
}

/// Turns a validated payload into the internal event, or `None` when a field
/// the event cannot do without was malformed (already logged by the lookup).
fn translate_event(payload: EventPayload) -> Option<ObsEvent> {
    let event = match payload.event_type.as_str() {
        "CurrentProgramSceneChanged" => ObsEvent::CurrentProgramSceneChanged {
            scene_name: payload.required_str("sceneName")?,
        },
        "SceneCreated" | "SceneRemoved" | "SceneNameChanged" | "SceneListReindexed" => {
            ObsEvent::SceneListChanged
        }
        "InputCreated" => ObsEvent::InputCreated {
            input_name: payload.required_str("inputName")?,
        },
        "InputRemoved" => ObsEvent::InputRemoved {
            input_name: payload.required_str("inputName")?,
        },
        "InputMuteStateChanged" => ObsEvent::InputMuteStateChanged {
            input_name: payload.required_str("inputName")?,
            muted: payload.required_bool("inputMuted")?,
        },
        "InputVolumeChanged" => ObsEvent::InputVolumeChanged {
            input_name: payload.required_str("inputName")?,
            volume_mul: payload.required_volume_multiplier("inputVolumeMul")?,
            volume_db: payload.required_decibels("inputVolumeDb")?,
        },
        "InputVolumeMeters" => ObsEvent::InputVolumeMeters {
            inputs: parse_volume_meters(&payload)?,
        },
        "StreamStateChanged" => ObsEvent::StreamStateChanged {
            active: payload.required_bool("outputActive")?,
        },
        "RecordStateChanged" => ObsEvent::RecordStateChanged {
            active: payload.required_bool("outputActive")?,
        },
        "CurrentProfileChanged" => ObsEvent::CurrentProfileChanged {
            profile_name: payload.required_str("profileName")?,
        },
        "ProfileListChanged" => ObsEvent::ProfileListChanged,
        "CurrentSceneCollectionChanged" => ObsEvent::CurrentSceneCollectionChanged {
            scene_collection_name: payload.required_str("sceneCollectionName")?,
        },
        "SceneCollectionListChanged" => ObsEvent::SceneCollectionListChanged,
        _ => ObsEvent::Other {
            event_type: payload.event_type.clone(),
            data: payload.data.clone(),
        },
    };
    Some(event)
}

/// Reduces the `InputVolumeMeters` payload to one peak level per input.
///
/// OBS emits a `[magnitude, peak, inputPeak]` tuple per audio channel, so the
/// level shown for an input is the loudest magnitude across its channels. A
/// meter with no channels yet — an input that has not run its first audio
/// capture callback — is a valid silent reading, not a malformed entry.
///
/// Any genuinely invalid entry discards the whole event rather than reporting a
/// partial set of levels that would look like some inputs went silent.
fn parse_volume_meters(payload: &EventPayload) -> Option<Vec<(String, f32)>> {
    let inputs = payload.required_array("inputs")?;
    let input_names = extract_resource_names(&payload.data, "inputs", "inputName")
        .map_err(|error| {
            warn!(
                event_type = %payload.event_type,
                message = %error,
                "Malformed OBS InputVolumeMeters payload: invalid inputName list"
            );
        })
        .ok()?;

    let mut had_invalid_entry = false;
    let mut levels_by_input: Vec<(String, f32)> = Vec::new();

    for (name, input) in input_names.into_iter().zip(inputs.iter()) {
        let channels = match input.get("inputLevelsMul") {
            Some(Value::Array(channels)) => channels,
            None => {
                had_invalid_entry = true;
                continue;
            }
            Some(invalid) => {
                warn!(
                    event_type = %payload.event_type,
                    input = %name,
                    data = %invalid,
                    "Malformed OBS InputVolumeMeters payload: inputLevelsMul must be an array"
                );
                had_invalid_entry = true;
                continue;
            }
        };

        let mut peak = 0.0_f32;
        for channel in channels {
            match channel
                .as_array()
                .and_then(|values| values.first())
                .and_then(|value| value.as_f64())
                .map(|value| value as f32)
            {
                Some(magnitude) if magnitude.is_finite() && magnitude >= 0.0 => {
                    peak = peak.max(magnitude);
                }
                _ => had_invalid_entry = true,
            }
        }

        levels_by_input.push((name, peak));
    }

    if had_invalid_entry {
        warn!(
            event_type = %payload.event_type,
            "Malformed OBS InputVolumeMeters payload: discarding event due to invalid entries"
        );
        return None;
    }
    Some(levels_by_input)
}

/// Probe a WebSocket message without error – used in fake server tests.
pub fn parse_ws_message(text: &str) -> Option<ObsMessage> {
    serde_json::from_str(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, timeout};

    #[test]
    fn obs_event_dispatches_scene_change() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": { "sceneName": "Main" }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.unwrap();
            match event {
                ObsEvent::CurrentProgramSceneChanged { scene_name } => {
                    assert_eq!(scene_name, "Main");
                }
                _ => panic!("wrong event type"),
            }
        });
    }

    #[test]
    fn obs_event_dispatches_profile_change() {
        let data = serde_json::json!({
            "eventType": "CurrentProfileChanged",
            "eventData": { "profileName": "Streaming" }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.unwrap();
            match event {
                ObsEvent::CurrentProfileChanged { profile_name } => {
                    assert_eq!(profile_name, "Streaming");
                }
                _ => panic!("wrong event type"),
            }
        });
    }

    #[test]
    fn obs_event_dispatches_profile_list_changed() {
        let data = serde_json::json!({
            "eventType": "ProfileListChanged",
            "eventData": {}
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.unwrap();
            assert!(matches!(event, ObsEvent::ProfileListChanged));
        });
    }

    #[test]
    fn obs_event_dispatches_scene_collection_change() {
        let data = serde_json::json!({
            "eventType": "CurrentSceneCollectionChanged",
            "eventData": { "sceneCollectionName": "Podcast" }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.unwrap();
            match event {
                ObsEvent::CurrentSceneCollectionChanged {
                    scene_collection_name,
                } => {
                    assert_eq!(scene_collection_name, "Podcast");
                }
                _ => panic!("wrong event type"),
            }
        });
    }

    #[test]
    fn obs_event_dispatches_scene_collection_list_changed() {
        let data = serde_json::json!({
            "eventType": "SceneCollectionListChanged",
            "eventData": {}
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.unwrap();
            assert!(matches!(event, ObsEvent::SceneCollectionListChanged));
        });
    }

    #[test]
    fn obs_event_dispatches_mute_change() {
        let data = serde_json::json!({
            "eventType": "InputMuteStateChanged",
            "eventData": { "inputName": "Mic", "inputMuted": true }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.unwrap();
            match event {
                ObsEvent::InputMuteStateChanged { input_name, muted } => {
                    assert_eq!(input_name, "Mic");
                    assert!(muted);
                }
                _ => panic!("wrong event type"),
            }
        });
    }

    #[test]
    fn obs_event_drops_invalid_scene_event() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": {}
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_invalid_stream_state() {
        let data = serde_json::json!({
            "eventType": "StreamStateChanged",
            "eventData": {}
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_invalid_volume_meters() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [ { "inputName": "Mic" } ]
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_volume_meters_with_invalid_channel_payload() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [
                    {
                        "inputName": "Mic",
                        "inputLevelsMul": [ [1.0], [0.5] ]
                    },
                    {
                        "inputName": "Music",
                        "inputLevelsMul": [ ["bad"] ]
                    }
                ]
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_scene_with_control_characters() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": { "sceneName": "Main\nScene" }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_volume_meters_with_invalid_name() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [
                    {
                        "inputName": "Mic\tLeft",
                        "inputLevelsMul": [ [1.0], [0.5] ]
                    }
                ]
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_forwards_volume_changed_with_negative_decibels() {
        // Decibel levels below zero are the normal case for anything quieter
        // than unity gain, so only the linear multiplier rejects negatives.
        let data = serde_json::json!({
            "eventType": "InputVolumeChanged",
            "eventData": {
                "inputName": "Mic",
                "inputVolumeMul": 0.25,
                "inputVolumeDb": -12.3
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = timeout(Duration::from_millis(50), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                event,
                ObsEvent::InputVolumeChanged {
                    input_name: "Mic".to_string(),
                    volume_mul: 0.25,
                    volume_db: -12.3,
                }
            );
        });
    }

    #[test]
    fn obs_event_drops_invalid_volume_changed_negative_mul() {
        let data = serde_json::json!({
            "eventType": "InputVolumeChanged",
            "eventData": {
                "inputName": "Mic",
                "inputVolumeMul": -0.25,
                "inputVolumeDb": -12.3
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_preserves_linear_meter_level() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [
                    { "inputName": "Mic", "inputLevelsMul": [[0.1, 0.2, 0.3]] }
                ]
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.expect("event should not be dropped");
            match event {
                ObsEvent::InputVolumeMeters { inputs } => {
                    assert_eq!(inputs.len(), 1);
                    assert_eq!(inputs[0].0, "Mic");
                    assert!((inputs[0].1 - 0.1).abs() < 1e-4, "got {}", inputs[0].1);
                }
                _ => panic!("wrong event type"),
            }
        });
    }

    #[test]
    fn obs_event_treats_empty_meter_channels_as_silence() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [
                    { "inputName": "Mic", "inputLevelsMul": [] }
                ]
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            let event = rx.recv().await.expect("event should not be dropped");
            match event {
                ObsEvent::InputVolumeMeters { inputs } => {
                    assert_eq!(inputs, vec![("Mic".to_string(), 0.0)]);
                }
                _ => panic!("wrong event type"),
            }
        });
    }

    #[test]
    fn obs_event_drops_volume_meters_with_duplicate_names() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [
                    {
                        "inputName": "Mic",
                        "inputLevelsMul": [[1.0]]
                    },
                    {
                        "inputName": "Mic",
                        "inputLevelsMul": [[0.5]]
                    }
                ]
            }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_invalid_event_type() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged\n"
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_oversize_event_type() {
        let event_type = "a".repeat(300);
        let data = serde_json::json!({
            "eventType": event_type,
            "eventData": { "sceneName": "Main" }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }

    #[test]
    fn obs_event_drops_oversize_string_field() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": { "sceneName": "a".repeat(300) }
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            assert!(timeout(Duration::from_millis(50), rx.recv()).await.is_err());
        });
    }
}
