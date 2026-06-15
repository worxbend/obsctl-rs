use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tracing::{debug, warn};

use crate::domain::{errors::ObsctlError, result::Result};
use crate::obs::protocol::{
    OPCODE_EVENT, OPCODE_IDENTIFY, OPCODE_REQUEST, OPCODE_REQUEST_RESPONSE, ObsMessage,
    RequestData, RequestResponseData,
};

/// Events emitted by the OBS client to its supervisor.
#[derive(Debug)]
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
}

impl ObsClient {
    /// Send a request and wait for the response data.
    pub async fn request(&self, req: RequestData) -> Result<Value> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.sender
            .send(ObsClientRequest {
                request: req,
                reply: reply_tx,
            })
            .await
            .map_err(|_| ObsctlError::ObsUnavailable)?;
        reply_rx.await.map_err(|_| ObsctlError::ObsUnavailable)?
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
/// Returns the client handle on success.
pub async fn handshake(
    mut sink: WsSink,
    mut stream: WsStream,
    password: Option<&str>,
    event_tx: mpsc::Sender<ObsEvent>,
) -> Result<(ObsClient, String, String)> {
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
            event_subscriptions: Some(13), // General=1, Scenes=4, Inputs=8
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

    // Spawn the client task
    let (req_tx, req_rx) = mpsc::channel::<ObsClientRequest>(64);
    tokio::spawn(run_client_task(sink, stream, req_rx, event_tx));

    let client = ObsClient { sender: req_tx };

    // Fetch OBS version to return metadata
    let version_data = client.request(crate::obs::requests::get_version()).await?;
    let obs_studio_version = version_data
        .get("obsVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok((client, obs_studio_version, obs_ws_version))
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
    event_tx: mpsc::Sender<ObsEvent>,
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
                    let _ = reply.send(Err(ObsctlError::ObsUnavailable));
                    for (_, s) in pending.drain() {
                        let _ = s.send(Err(ObsctlError::ObsUnavailable));
                    }
                    break;
                }
                pending.insert(id, reply);
            }

            maybe_msg = stream.next() => {
                match maybe_msg {
                    None => {
                        for (_, s) in pending.drain() {
                            let _ = s.send(Err(ObsctlError::ObsUnavailable));
                        }
                        break;
                    }
                    Some(Err(_)) => {
                        for (_, s) in pending.drain() {
                            let _ = s.send(Err(ObsctlError::ObsUnavailable));
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

async fn dispatch_event(data: Value, event_tx: &mpsc::Sender<ObsEvent>) {
    let event_type = data
        .get("eventType")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let event_data = data.get("eventData").cloned().unwrap_or(Value::Null);

    let event = match event_type.as_str() {
        "CurrentProgramSceneChanged" => ObsEvent::CurrentProgramSceneChanged {
            scene_name: event_data
                .get("sceneName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        "SceneCreated" | "SceneRemoved" | "SceneNameChanged" | "SceneListReindexed" => {
            ObsEvent::SceneListChanged
        }
        "InputCreated" => ObsEvent::InputCreated {
            input_name: event_data
                .get("inputName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        "InputRemoved" => ObsEvent::InputRemoved {
            input_name: event_data
                .get("inputName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        "InputMuteStateChanged" => ObsEvent::InputMuteStateChanged {
            input_name: event_data
                .get("inputName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            muted: event_data
                .get("inputMuted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        },
        "InputVolumeChanged" => ObsEvent::InputVolumeChanged {
            input_name: event_data
                .get("inputName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            volume_mul: event_data
                .get("inputVolumeMul")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            volume_db: event_data
                .get("inputVolumeDb")
                .and_then(|v| v.as_f64())
                .unwrap_or(f64::NEG_INFINITY),
        },
        _ => ObsEvent::Other {
            event_type,
            data: event_data,
        },
    };

    let _ = event_tx.send(event).await;
}

/// Probe a WebSocket message without error – used in fake server tests.
pub fn parse_ws_message(text: &str) -> Option<ObsMessage> {
    serde_json::from_str(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
