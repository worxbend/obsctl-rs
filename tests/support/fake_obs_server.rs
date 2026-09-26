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
use tokio::sync::{Mutex, Semaphore, broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{WebSocketStream, accept_async};

const OPCODE_HELLO: u8 = 0;
const OPCODE_IDENTIFY: u8 = 1;
const OPCODE_IDENTIFIED: u8 = 2;
const OPCODE_REQUEST: u8 = 6;
const OPCODE_REQUEST_RESPONSE: u8 = 7;
const OPCODE_EVENT: u8 = 5;

const AUTH_SALT: &str = "PZVbYpvAnZut2SS3k3tnTQ==";
const AUTH_CHALLENGE: &str = "lfYW3AhFLp2YcILmwSQ9rSFRIiEQgxuEk5hSyQ3XGaQ=";

type WsSink = futures_util::stream::SplitSink<WebSocketStream<TcpStream>, Message>;
type WsSource = futures_util::stream::SplitStream<WebSocketStream<TcpStream>>;

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
    /// When set, the response waits here until the test releases it — see
    /// [`PreparedResponse::gated`].
    gate: Option<Arc<Semaphore>>,
}

impl PreparedResponse {
    pub fn success(data: Value) -> Self {
        Self {
            ok: true,
            data,
            comment: None,
            no_reply: false,
            delay: None,
            gate: None,
        }
    }

    pub fn error(comment: &str) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            comment: Some(comment.to_string()),
            no_reply: false,
            delay: None,
            gate: None,
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
            gate: None,
        }
    }

    /// The server receives the request and sends this response only after `delay`.
    pub fn delayed(mut self, delay: std::time::Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// The server receives the request and holds the reply until the test says
    /// so, through the returned [`ResponseGate`].
    ///
    /// This is how a test makes a command take a controllable amount of time
    /// inside the daemon without timing anything: the command is stuck in its
    /// OBS round trip for exactly as long as the test leaves the gate shut, so
    /// there is no delay to guess at and no race to lose.
    ///
    /// The connection handling the gated request waits with it, so any *other*
    /// request on the same connection is held too. A test that needs something
    /// to happen while the gate is shut must therefore choose something that
    /// does not go to OBS at all.
    pub fn gated(mut self) -> (Self, ResponseGate) {
        let gate = Arc::new(Semaphore::new(0));
        self.gate = Some(Arc::clone(&gate));
        (self, ResponseGate(gate))
    }
}

/// Releases responses held by [`PreparedResponse::gated`], one per call.
#[derive(Clone)]
pub struct ResponseGate(Arc<Semaphore>);

impl ResponseGate {
    /// Let one held request answer.
    pub fn release(&self) {
        self.0.add_permits(1);
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

/// Handle to a fake OBS that never speaks the protocol — see [`spawn_silent_obs`].
pub struct SilentObsHandle {
    pub addr: SocketAddr,
    /// One item per client that finished the WebSocket upgrade.
    connections: mpsc::Receiver<()>,
    shutdown: oneshot::Sender<()>,
}

impl SilentObsHandle {
    /// Resolve once a client has completed the WebSocket upgrade against this
    /// server, so a test knows the connection attempt it is about to interfere
    /// with is genuinely in flight — without sleeping to guess at it.
    pub async fn wait_for_connection(&mut self) {
        self.connections
            .recv()
            .await
            .expect("silent fake OBS accepted no connection");
    }

    /// Stop accepting, and drop every connection being held open.
    pub fn shutdown(self) {
        let _ = self.shutdown.send(());
    }
}

/// Spawn a fake OBS that accepts the TCP connection and the WebSocket upgrade
/// and then deliberately says nothing at all — no Hello, ever.
///
/// This is the "hung OBS" shape: from the client's point of view the socket is
/// open and healthy, so the connect step succeeds and the obs-websocket
/// handshake is left waiting for a message that never comes. The connection is
/// held open (rather than dropped) so the client waits rather than seeing a
/// close frame.
pub async fn spawn_silent_obs() -> SilentObsHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (connection_tx, connection_rx) = mpsc::channel::<()>(8);
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        // Every upgraded connection is parked here, keeping the WebSocket
        // alive; they are all dropped together when this task ends.
        let mut held = Vec::new();
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                result = listener.accept() => {
                    let Ok((stream, _)) = result else { break };
                    let Ok(ws_stream) = accept_async(stream).await else { continue };
                    held.push(ws_stream);
                    if connection_tx.send(()).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    SilentObsHandle {
        addr,
        connections: connection_rx,
        shutdown: shutdown_tx,
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
    let password = password.map(str::to_string);
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
    let Ok(ws_stream) = accept_async(stream).await else {
        return;
    };

    let (mut sink, mut source) = ws_stream.split();

    if run_handshake(&mut sink, &mut source, require_auth, password.as_deref())
        .await
        .is_none()
    {
        return;
    }

    drain_pending_events(&mut sink, &state).await;
    serve_requests(
        &mut sink,
        &mut source,
        &state,
        &req_tx,
        &mut disconnect_rx,
        &mut event_rx,
    )
    .await;
}

/// Run the obs-websocket 5.x handshake — Hello, Identify, optional auth
/// check, Identified. `None` means the connection is finished.
async fn run_handshake(
    sink: &mut WsSink,
    source: &mut WsSource,
    require_auth: bool,
    password: Option<&str>,
) -> Option<()> {
    if sink
        .send(Message::Text(hello_message(require_auth).to_string()))
        .await
        .is_err()
    {
        return None;
    }

    let identify = receive_identify(source).await?;
    if identify.get("op").and_then(|v| v.as_u64()) != Some(OPCODE_IDENTIFY as u64) {
        return None;
    }

    if !auth_accepted(&identify, require_auth, password) {
        // Close without Identified — the client will fail.
        let _ = sink.send(Message::Close(None)).await;
        return None;
    }

    let identified = json!({
        "op": OPCODE_IDENTIFIED,
        "d": { "negotiatedRpcVersion": 1 }
    });
    sink.send(Message::Text(identified.to_string()))
        .await
        .ok()?;
    Some(())
}

fn hello_message(require_auth: bool) -> Value {
    if require_auth {
        json!({
            "op": OPCODE_HELLO,
            "d": {
                "obsWebSocketVersion": "5.0.0",
                "rpcVersion": 1,
                "authentication": {
                    "challenge": AUTH_CHALLENGE,
                    "salt": AUTH_SALT,
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
    }
}

async fn receive_identify(source: &mut WsSource) -> Option<Value> {
    let raw = loop {
        match source.next().await {
            Some(Ok(Message::Text(t))) => break t,
            Some(Ok(Message::Binary(b))) => break String::from_utf8(b).unwrap_or_default(),
            Some(Ok(_)) => continue,
            _ => return None,
        }
    };
    serde_json::from_str(&raw).ok()
}

fn auth_accepted(identify: &Value, require_auth: bool, password: Option<&str>) -> bool {
    if !require_auth {
        return true;
    }
    let Some(pw) = password else {
        return true;
    };
    let provided_auth = identify
        .get("d")
        .and_then(|d| d.get("authentication"))
        .and_then(|a| a.as_str())
        .unwrap_or("");
    let expected = obsctl_rs::obs::auth::compute_authentication(pw, AUTH_SALT, AUTH_CHALLENGE);
    provided_auth == expected
}

/// Push any events queued before this connection identified.
async fn drain_pending_events(sink: &mut WsSink, state: &Arc<Mutex<ServerState>>) {
    let mut st = state.lock().await;
    for event_data in st.pending_events.drain(..) {
        let event_msg = json!({
            "op": OPCODE_EVENT,
            "d": event_data,
        });
        let _ = sink.send(Message::Text(event_msg.to_string())).await;
    }
}

async fn serve_requests(
    sink: &mut WsSink,
    source: &mut WsSource,
    state: &Arc<Mutex<ServerState>>,
    req_tx: &mpsc::Sender<(String, Value)>,
    disconnect_rx: &mut broadcast::Receiver<()>,
    event_rx: &mut broadcast::Receiver<Value>,
) {
    loop {
        tokio::select! {
            event = event_rx.recv() => {
                if !forward_event(sink, event).await {
                    break;
                }
            }
            _ = disconnect_rx.recv() => {
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            msg = source.next() => {
                if !handle_incoming(sink, msg, state, req_tx).await {
                    break;
                }
            }
        }
    }
}

/// Forward a broadcast event to this connection; `false` ends the connection.
async fn forward_event(
    sink: &mut WsSink,
    event: Result<Value, broadcast::error::RecvError>,
) -> bool {
    let event_data = match event {
        Ok(event_data) => event_data,
        Err(broadcast::error::RecvError::Lagged(_)) => return true,
        Err(broadcast::error::RecvError::Closed) => return false,
    };
    let event_msg = json!({
        "op": OPCODE_EVENT,
        "d": event_data,
    });
    sink.send(Message::Text(event_msg.to_string()))
        .await
        .is_ok()
}

/// Handle one message from the client; `false` ends the connection.
async fn handle_incoming(
    sink: &mut WsSink,
    msg: Option<Result<Message, WsError>>,
    state: &Arc<Mutex<ServerState>>,
    req_tx: &mpsc::Sender<(String, Value)>,
) -> bool {
    let msg = match msg {
        Some(Ok(m)) => m,
        _ => return false,
    };
    let text = match msg {
        Message::Text(t) => t,
        Message::Binary(b) => String::from_utf8(b).unwrap_or_default(),
        Message::Close(_) => return false,
        _ => return true,
    };

    let request: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return true,
    };

    if request.get("op").and_then(|v| v.as_u64()) != Some(OPCODE_REQUEST as u64) {
        return true;
    }

    let Some(d) = request.get("d") else {
        return true;
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

    let prepared = state.lock().await.responses.get(&request_type).cloned();

    // If the prepared response says no_reply, drop the request silently.
    if prepared.as_ref().is_some_and(|p| p.no_reply) {
        return true;
    }

    let Some(response) = build_response(prepared, &request_type, &request_id).await else {
        return false;
    };

    sink.send(Message::Text(response.to_string())).await.is_ok()
}

/// Wait out the prepared delay/gate and render the response frame; `None`
/// means the response gate was dropped, which ends the connection.
async fn build_response(
    prepared: Option<PreparedResponse>,
    request_type: &str,
    request_id: &str,
) -> Option<Value> {
    let Some(p) = prepared else {
        // Default: success with empty data for known types
        let default_data = default_response(request_type);
        return Some(json!({
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
        }));
    };

    if let Some(delay) = p.delay {
        tokio::time::sleep(delay).await;
    }

    if let Some(gate) = &p.gate {
        // One permit per held request: the request stays here until the test
        // calls `ResponseGate::release`.
        match gate.acquire().await {
            Ok(permit) => permit.forget(),
            Err(_) => return None,
        }
    }

    Some(json!({
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
    }))
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
        "GetSceneCollectionList" => json!({
            "currentSceneCollectionName": "Podcast",
            "sceneCollections": ["Podcast", "Gaming"],
        }),
        _ => Value::Null,
    }
}
