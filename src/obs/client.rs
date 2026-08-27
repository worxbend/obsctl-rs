use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, warn};

use crate::domain::{errors::ObsctlError, result::Result};
use crate::obs::event_decoder::{EventPayload, translate_event};
use crate::obs::protocol::{
    HelloData, IdentifyData, OPCODE_EVENT, OPCODE_HELLO, OPCODE_IDENTIFIED, OPCODE_IDENTIFY,
    OPCODE_REQUEST, OPCODE_REQUEST_RESPONSE, ObsMessage, RequestData, RequestResponseData,
};

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
    let hello = read_hello(&mut stream).await?;
    let obs_ws_version = hello.obs_web_socket_version.clone();

    let identify = build_identify(&hello, password)?;
    let identify_json = serde_json::to_string(&identify)
        .map_err(|e| ObsctlError::ConnectionFailed(format!("serialize: {e}")))?;
    sink.send(Message::Text(identify_json))
        .await
        .map_err(|e| ObsctlError::ConnectionFailed(e.to_string()))?;

    await_identified(&mut stream).await?;

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

/// Read the first frame of the handshake, which obs-websocket guarantees is a
/// `Hello` describing the server and, when a password is set, the auth challenge.
async fn read_hello(stream: &mut WsStream) -> Result<HelloData> {
    let hello_raw = read_ws_message(stream).await?;
    if hello_raw.op != OPCODE_HELLO {
        return Err(ObsctlError::ConnectionFailed(format!(
            "expected Hello (opcode {}), got {}",
            OPCODE_HELLO, hello_raw.op
        )));
    }
    serde_json::from_value(hello_raw.d)
        .map_err(|e| ObsctlError::ConnectionFailed(format!("invalid Hello: {e}")))
}

/// Build the `Identify` reply to a `Hello`.
///
/// When the `Hello` carried an `authentication` block, OBS has a password set
/// and we must answer its challenge; failing to have one configured is an
/// authentication failure rather than a connection failure. When it did not,
/// the `authentication` field is sent as null.
fn build_identify(hello: &HelloData, password: Option<&str>) -> Result<ObsMessage> {
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

    Ok(ObsMessage {
        op: OPCODE_IDENTIFY,
        d: serde_json::to_value(IdentifyData {
            rpc_version: 1,
            authentication,
            event_subscriptions: Some(EVENT_SUBSCRIPTIONS),
        })
        .map_err(|e| ObsctlError::ConnectionFailed(format!("serialize Identify: {e}")))?,
    })
}

/// Wait for OBS to accept the `Identify`. Anything other than `Identified`
/// means the credentials were not accepted.
async fn await_identified(stream: &mut WsStream) -> Result<()> {
    let identified_raw = read_ws_message(stream).await?;
    if identified_raw.op != OPCODE_IDENTIFIED {
        return Err(ObsctlError::AuthenticationFailed);
    }
    Ok(())
}

/// Decode one WebSocket frame into an OBS message.
///
/// `None` means the frame carried no OBS payload at all (a keep-alive Ping or
/// Pong, or a raw frame), so the caller should simply read the next one.
/// `Some(Err(..))` means the connection is unusable — it closed, or the payload
/// was not a parsable OBS message.
fn decode_frame(msg: Message) -> Option<Result<ObsMessage>> {
    let parsed = match msg {
        Message::Text(text) => serde_json::from_str::<ObsMessage>(&text),
        Message::Binary(bin) => serde_json::from_slice::<ObsMessage>(&bin),
        Message::Close(_) => {
            return Some(Err(ObsctlError::ConnectionFailed(
                "WebSocket closed by peer".to_string(),
            )));
        }
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => return None,
    };
    Some(parsed.map_err(|e| ObsctlError::ConnectionFailed(format!("parse obs message: {e}"))))
}

pub(crate) async fn read_ws_message(stream: &mut WsStream) -> Result<ObsMessage> {
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(frame) => {
                if let Some(decoded) = decode_frame(frame) {
                    return decoded;
                }
            }
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
    let obs_msg = match decode_frame(msg) {
        Some(Ok(m)) => m,
        Some(Err(e)) => {
            warn!("Failed to parse OBS message: {e}");
            return;
        }
        None => return,
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

async fn dispatch_event(data: Value, event_tx: &mpsc::Sender<ObsEvent>) {
    let Some(payload) = EventPayload::parse(data) else {
        return;
    };
    let Some(event) = translate_event(payload) else {
        return;
    };
    let _ = event_tx.send(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::protocol::HelloAuthentication;
    use tokio::time::{Duration, timeout};

    fn hello(authentication: Option<HelloAuthentication>) -> HelloData {
        HelloData {
            obs_web_socket_version: "5.4.2".to_string(),
            rpc_version: 1,
            authentication,
        }
    }

    #[test]
    fn identify_omits_authentication_when_obs_has_no_password() {
        let identify = build_identify(&hello(None), None).unwrap();
        assert_eq!(identify.op, OPCODE_IDENTIFY);
        assert_eq!(identify.d["authentication"], Value::Null);
        assert_eq!(identify.d["rpcVersion"], 1);
        assert_eq!(identify.d["eventSubscriptions"], EVENT_SUBSCRIPTIONS);
    }

    #[test]
    fn identify_answers_the_challenge_when_obs_has_a_password() {
        let challenge = HelloAuthentication {
            challenge: "challenge".to_string(),
            salt: "salt".to_string(),
        };
        let expected = crate::obs::auth::compute_authentication("hunter2", "salt", "challenge");

        let identify = build_identify(&hello(Some(challenge)), Some("hunter2")).unwrap();
        assert_eq!(identify.d["authentication"], Value::String(expected));
    }

    #[test]
    fn identify_fails_when_obs_wants_a_password_and_none_is_configured() {
        let challenge = HelloAuthentication {
            challenge: "challenge".to_string(),
            salt: "salt".to_string(),
        };
        assert!(matches!(
            build_identify(&hello(Some(challenge)), None),
            Err(ObsctlError::AuthenticationFailed)
        ));
    }

    /// Run `dispatch_event` on `data` and return the event it forwarded, or
    /// `None` when the payload was dropped. Each of the tests below used to
    /// hand-roll a Tokio runtime, a channel, and a receive timeout to observe
    /// this one function; the plumbing lives here instead.
    fn dispatched(data: Value) -> Option<ObsEvent> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (tx, mut rx) = mpsc::channel(8);
            dispatch_event(data, &tx).await;
            timeout(Duration::from_millis(50), rx.recv())
                .await
                .ok()
                .flatten()
        })
    }

    #[test]
    fn obs_event_dispatches_scene_change() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": { "sceneName": "Main" }
        });
        assert_eq!(
            dispatched(data),
            Some(ObsEvent::CurrentProgramSceneChanged {
                scene_name: "Main".to_string(),
            })
        );
    }

    #[test]
    fn obs_event_dispatches_profile_change() {
        let data = serde_json::json!({
            "eventType": "CurrentProfileChanged",
            "eventData": { "profileName": "Streaming" }
        });
        assert_eq!(
            dispatched(data),
            Some(ObsEvent::CurrentProfileChanged {
                profile_name: "Streaming".to_string(),
            })
        );
    }

    #[test]
    fn obs_event_dispatches_profile_list_changed() {
        let data = serde_json::json!({
            "eventType": "ProfileListChanged",
            "eventData": {}
        });
        assert_eq!(dispatched(data), Some(ObsEvent::ProfileListChanged));
    }

    #[test]
    fn obs_event_dispatches_scene_collection_change() {
        let data = serde_json::json!({
            "eventType": "CurrentSceneCollectionChanged",
            "eventData": { "sceneCollectionName": "Podcast" }
        });
        assert_eq!(
            dispatched(data),
            Some(ObsEvent::CurrentSceneCollectionChanged {
                scene_collection_name: "Podcast".to_string(),
            })
        );
    }

    #[test]
    fn obs_event_dispatches_scene_collection_list_changed() {
        let data = serde_json::json!({
            "eventType": "SceneCollectionListChanged",
            "eventData": {}
        });
        assert_eq!(dispatched(data), Some(ObsEvent::SceneCollectionListChanged));
    }

    #[test]
    fn obs_event_dispatches_mute_change() {
        let data = serde_json::json!({
            "eventType": "InputMuteStateChanged",
            "eventData": { "inputName": "Mic", "inputMuted": true }
        });
        assert_eq!(
            dispatched(data),
            Some(ObsEvent::InputMuteStateChanged {
                input_name: "Mic".to_string(),
                muted: true,
            })
        );
    }

    #[test]
    fn obs_event_drops_invalid_scene_event() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": {}
        });
        assert_eq!(dispatched(data), None);
    }

    #[test]
    fn obs_event_drops_invalid_stream_state() {
        let data = serde_json::json!({
            "eventType": "StreamStateChanged",
            "eventData": {}
        });
        assert_eq!(dispatched(data), None);
    }

    #[test]
    fn obs_event_drops_invalid_volume_meters() {
        let data = serde_json::json!({
            "eventType": "InputVolumeMeters",
            "eventData": {
                "inputs": [ { "inputName": "Mic" } ]
            }
        });
        assert_eq!(dispatched(data), None);
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
        assert_eq!(dispatched(data), None);
    }

    #[test]
    fn obs_event_drops_scene_with_control_characters() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": { "sceneName": "Main\nScene" }
        });
        assert_eq!(dispatched(data), None);
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
        assert_eq!(dispatched(data), None);
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
        assert_eq!(
            dispatched(data),
            Some(ObsEvent::InputVolumeChanged {
                input_name: "Mic".to_string(),
                volume_mul: 0.25,
                volume_db: -12.3,
            })
        );
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
        assert_eq!(dispatched(data), None);
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
        match dispatched(data).expect("event should not be dropped") {
            ObsEvent::InputVolumeMeters { inputs } => {
                assert_eq!(inputs.len(), 1);
                assert_eq!(inputs[0].0, "Mic");
                assert!((inputs[0].1 - 0.1).abs() < 1e-4, "got {}", inputs[0].1);
            }
            _ => panic!("wrong event type"),
        }
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
        assert_eq!(
            dispatched(data),
            Some(ObsEvent::InputVolumeMeters {
                inputs: vec![("Mic".to_string(), 0.0)],
            })
        );
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
        assert_eq!(dispatched(data), None);
    }

    #[test]
    fn obs_event_drops_invalid_event_type() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged\n"
        });
        assert_eq!(dispatched(data), None);
    }

    #[test]
    fn obs_event_drops_oversize_event_type() {
        let event_type = "a".repeat(300);
        let data = serde_json::json!({
            "eventType": event_type,
            "eventData": { "sceneName": "Main" }
        });
        assert_eq!(dispatched(data), None);
    }

    #[test]
    fn obs_event_drops_oversize_string_field() {
        let data = serde_json::json!({
            "eventType": "CurrentProgramSceneChanged",
            "eventData": { "sceneName": "a".repeat(300) }
        });
        assert_eq!(dispatched(data), None);
    }
}
