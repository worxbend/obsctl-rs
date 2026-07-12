// Fake OBS WebSocket server for integration tests — Phase 4.
//
// Binds a random local port, runs the obs-websocket 5.x handshake, and
// dispatches Request messages to a configurable handler. Tests control the
// server through the returned `FakeObsHandle`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

const OPCODE_HELLO: u8 = 0;
const OPCODE_IDENTIFY: u8 = 1;
const OPCODE_IDENTIFIED: u8 = 2;
const OPCODE_REQUEST: u8 = 6;
const OPCODE_REQUEST_RESPONSE: u8 = 7;
const OPCODE_EVENT: u8 = 5;

/// A prepared response for a given requestType.
#[derive(Clone)]
pub struct PreparedResponse {
    pub ok: bool,
    pub data: Value,
    pub comment: Option<String>,
    /// When true the server silently discards the request without sending any reply.
    pub no_reply: bool,
    /// Delay before sending the response, used to simulate a late OBS reply.
    pub delay: Option<std::time::Duration>,
}

impl PreparedResponse {
    pub fn success(data: Value) -> Self {
        Self {
            ok: true,
            data,
            comment: None,
            no_reply: false,
            delay: None,
        }
    }

    pub fn error(comment: &str) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            comment: Some(comment.to_string()),
            no_reply: false,
            delay: None,
        }
    }

    /// The server receives the request but never sends a response, simulating a stalled OBS.
    pub fn no_reply() -> Self {
        Self {
            ok: false,
            data: Value::Null,
            comment: None,
            no_reply: true,
            delay: None,
        }
    }

    /// The server receives the request and sends this response only after `delay`.
    pub fn delayed(mut self, delay: std::time::Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

struct ServerState {
    /// Per requestType response overrides.
    responses: HashMap<String, PreparedResponse>,
    /// Events to push once identified.
    pending_events: Vec<Value>,
}

/// Handle returned to tests.
pub struct FakeObsHandle {
    pub addr: SocketAddr,
    state: Arc<Mutex<ServerState>>,
    /// Receives captured requests for assertion.
    pub requests: mpsc::Receiver<(String, Value)>,
    shutdown: oneshot::Sender<()>,
    /// Broadcast to disconnect all active connection handlers.
    disconnect_tx: broadcast::Sender<()>,
    /// Broadcast OBS events to all active connection handlers.
    event_tx: broadcast::Sender<Value>,
}

impl FakeObsHandle {
    /// Override the response for a given requestType.
    pub async fn set_response(&self, request_type: &str, response: PreparedResponse) {
        self.state
            .lock()
            .await
            .responses
            .insert(request_type.to_string(), response);
    }

    /// Queue an OBS event to push to the client after identification.
    pub async fn push_event(&self, event_type: &str, event_data: Value) {
        let msg = json!({
            "eventType": event_type,
            "eventData": event_data,
        });
        self.state.lock().await.pending_events.push(msg);
    }

    /// Emit an OBS event to currently connected clients.
    pub fn emit_event(&self, event_type: &str, event_data: Value) {
        let msg = json!({
            "eventType": event_type,
            "eventData": event_data,
        });
        let _ = self.event_tx.send(msg);
    }

    /// Close all active WebSocket connections without stopping the accept loop.
    pub fn disconnect_all(&self) {
        let _ = self.disconnect_tx.send(());
    }

    /// Shut down the fake server (closes the listener and all connections).
    pub fn shutdown(self) {
        let _ = self.disconnect_tx.send(());
        let _ = self.shutdown.send(());
    }
}

/// Spawn a fake OBS server on a random local port.
/// `require_auth` — if true the server sends a Hello with challenge/salt.
pub async fn spawn_fake_obs(require_auth: bool, password: Option<&str>) -> FakeObsHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let state = Arc::new(Mutex::new(ServerState {
        responses: HashMap::new(),
        pending_events: Vec::new(),
    }));

    let (req_tx, req_rx) = mpsc::channel::<(String, Value)>(64);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let (disconnect_tx, _) = broadcast::channel::<()>(8);
    let (event_tx, _) = broadcast::channel::<Value>(16);

    let state_clone = state.clone();
    let password = password.map(|p| p.to_string());
    let disconnect_tx_clone = disconnect_tx.clone();
    let event_tx_clone = event_tx.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                result = listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let s = state_clone.clone();
                            let tx = req_tx.clone();
                            let pw = password.clone();
                            let auth = require_auth;
                            let disc_rx = disconnect_tx_clone.subscribe();
                            let event_rx = event_tx_clone.subscribe();
                            tokio::spawn(handle_connection(stream, s, tx, auth, pw, disc_rx, event_rx));
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    });

    FakeObsHandle {
        addr,
        state,
        requests: req_rx,
        shutdown: shutdown_tx,
        disconnect_tx,
        event_tx,
    }
}

async fn handle_connection(
    stream: TcpStream,
    state: Arc<Mutex<ServerState>>,
    req_tx: mpsc::Sender<(String, Value)>,
    require_auth: bool,
    password: Option<String>,
    mut disconnect_rx: broadcast::Receiver<()>,
    mut event_rx: broadcast::Receiver<Value>,
) {
    let ws_stream = match accept_async(stream).await {
        Ok(s) => s,
        Err(_) => return,
    };

    let (mut sink, mut source) = ws_stream.split();

    // --- Send Hello ---
    let salt = "PZVbYpvAnZut2SS3k3tnTQ==";
    let challenge = "lfYW3AhFLp2YcILmwSQ9rSFRIiEQgxuEk5hSyQ3XGaQ=";

    let hello = if require_auth {
        json!({
            "op": OPCODE_HELLO,
            "d": {
                "obsWebSocketVersion": "5.0.0",
                "rpcVersion": 1,
                "authentication": {
                    "challenge": challenge,
                    "salt": salt,
                }
            }
        })
    } else {
        json!({
            "op": OPCODE_HELLO,
            "d": {
                "obsWebSocketVersion": "5.0.0",
                "rpcVersion": 1,
            }
        })
    };

    if sink.send(Message::Text(hello.to_string())).await.is_err() {
        return;
    }

    // --- Receive Identify ---
    let identify_msg = loop {
        match source.next().await {
            Some(Ok(Message::Text(t))) => break t,
            Some(Ok(Message::Binary(b))) => {
                break String::from_utf8(b).unwrap_or_default();
            }
            Some(Ok(_)) => continue,
            _ => return,
        }
    };

    let identify: Value = match serde_json::from_str(&identify_msg) {
        Ok(v) => v,
        Err(_) => return,
    };

    if identify.get("op").and_then(|v| v.as_u64()) != Some(OPCODE_IDENTIFY as u64) {
        return;
    }

    // Check auth if required
    if require_auth {
        let provided_auth = identify
            .get("d")
            .and_then(|d| d.get("authentication"))
            .and_then(|a| a.as_str())
            .unwrap_or("");

        if let Some(ref pw) = password {
            let expected = obsctl_rs::obs::auth::compute_authentication(pw, salt, challenge);
            if provided_auth != expected {
                // Send close without Identified — client will fail
                let _ = sink.send(Message::Close(None)).await;
                return;
            }
        }
    }

    // --- Send Identified ---
    let identified = json!({
        "op": OPCODE_IDENTIFIED,
        "d": { "negotiatedRpcVersion": 1 }
    });
    if sink
        .send(Message::Text(identified.to_string()))
        .await
        .is_err()
    {
        return;
    }

    // Push any queued events
    {
        let mut st = state.lock().await;
        for event_data in st.pending_events.drain(..) {
            let event_msg = json!({
                "op": OPCODE_EVENT,
                "d": event_data,
            });
            let _ = sink.send(Message::Text(event_msg.to_string())).await;
        }
    }

    // --- Handle requests ---
    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let event_data = match event {
                    Ok(event_data) => event_data,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                let event_msg = json!({
                    "op": OPCODE_EVENT,
                    "d": event_data,
                });
                if sink
                    .send(Message::Text(event_msg.to_string()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            _ = disconnect_rx.recv() => {
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            msg = source.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    _ => break,
                };
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Binary(b) => String::from_utf8(b).unwrap_or_default(),
                    Message::Close(_) => break,
                    _ => continue,
                };

                let request: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if request.get("op").and_then(|v| v.as_u64()) != Some(OPCODE_REQUEST as u64) {
                    continue;
                }

                let d = match request.get("d") {
                    Some(d) => d,
                    None => continue,
                };

                let request_type = d
                    .get("requestType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let request_id = d
                    .get("requestId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let request_data = d.get("requestData").cloned().unwrap_or(Value::Null);

                // Record the request
                let _ = req_tx.send((request_type.clone(), request_data)).await;

                // Build response
                let prepared = state.lock().await.responses.get(&request_type).cloned();

                // If the prepared response says no_reply, drop the request silently.
                if prepared.as_ref().is_some_and(|p| p.no_reply) {
                    continue;
                }

                let response = if let Some(p) = prepared {
                    if let Some(delay) = p.delay {
                        tokio::time::sleep(delay).await;
                    }

                    json!({
                        "op": OPCODE_REQUEST_RESPONSE,
                        "d": {
                            "requestType": request_type,
                            "requestId": request_id,
                            "requestStatus": {
                                "result": p.ok,
                                "code": if p.ok { 100u32 } else { 400u32 },
                                "comment": p.comment,
                            },
                            "responseData": p.data,
                        }
                    })
                } else {
                    // Default: success with empty data for known types
                    let default_data = default_response(&request_type);
                    json!({
                        "op": OPCODE_REQUEST_RESPONSE,
                        "d": {
                            "requestType": request_type,
                            "requestId": request_id,
                            "requestStatus": {
                                "result": true,
                                "code": 100,
                            },
                            "responseData": default_data,
                        }
                    })
                };

                if sink
                    .send(Message::Text(response.to_string()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}

fn default_response(request_type: &str) -> Value {
    match request_type {
        "GetVersion" => json!({
            "obsVersion": "30.0.0",
            "obsWebSocketVersion": "5.0.0",
            "rpcVersion": 1,
        }),
        "GetSceneList" => json!({
            "currentProgramSceneName": "Main",
            "scenes": [
                { "sceneName": "Main", "sceneIndex": 0 },
                { "sceneName": "BRB", "sceneIndex": 1 },
            ]
        }),
        "GetCurrentProgramScene" => json!({
            "currentProgramSceneName": "Main",
        }),
        "GetInputList" => json!({
            "inputs": [
                { "inputName": "Mic", "inputKind": "alsa_input_capture" },
                { "inputName": "Desktop", "inputKind": "pulse_output_capture" },
            ]
        }),
        "GetInputMute" => json!({ "inputMuted": false }),
        "GetInputVolume" => json!({
            "inputVolumeMul": 1.0,
            "inputVolumeDb": 0.0,
        }),
        "GetStats" => json!({
            "cpuUsage": 12.5,
            "memoryUsage": 512.0,
            "availableDiskSpace": 100_000.0,
            "activeFps": 60.0,
            "averageFrameRenderTime": 3.2,
            "renderSkippedFrames": 0,
            "renderTotalFrames": 1000,
            "outputSkippedFrames": 0,
            "outputTotalFrames": 1000,
        }),
        "GetProfileList" => json!({
            "currentProfileName": "Default",
            "profiles": ["Default", "Streaming"],
        }),
        _ => Value::Null,
    }
}
