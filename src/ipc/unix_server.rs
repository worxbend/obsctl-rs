use std::{path::Path, sync::Arc};

use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{broadcast::error::RecvError, mpsc, oneshot, watch},
};
use tracing::{debug, error, warn};

use crate::ipc::{
    codec::{FrameEncodeError, FrameError, FrameReader, decode, encode_framed},
    protocol::{
        ClientMessage, CommandPayload, ErrorPayload, MAX_IPC_LINE_BYTES, PublicErrorCode,
        ServerCommand, ServerMessage, Topic, normalize_subscribe_topics, validate_command_name,
        validate_ipc_request_id,
    },
    session::{BroadcastHub, CommandDispatch, CommandLanes, SessionSubscriptions},
    socket_path::ensure_socket_file,
};
use crate::server::client_registry::ClientRegistry;
use crate::support::fs;

pub struct IpcServer {
    listener: UnixListener,
    hub: Arc<BroadcastHub>,
    registry: ClientRegistry,
}

const STATE_INIT_REQUEST_ID: &str = "state-init";

/// What the read loop should do after handling one client frame.
///
/// The handlers below decide both what to answer and whether the connection
/// survives the answer; this names that second decision so a caller reads
/// `SessionControl::Close` instead of an unexplained `false`.
#[derive(Debug, PartialEq, Eq)]
enum SessionControl {
    Continue,
    Close,
}

impl IpcServer {
    pub fn bind(path: &Path, hub: Arc<BroadcastHub>) -> std::io::Result<Self> {
        Self::bind_with_registry(path, hub, ClientRegistry::new())
    }

    pub fn bind_with_registry(
        path: &Path,
        hub: Arc<BroadcastHub>,
        registry: ClientRegistry,
    ) -> std::io::Result<Self> {
        if let Err(error) = crate::ipc::socket_path::validate_socket_path(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsafe socket path: {error}"),
            ));
        }

        crate::ipc::socket_path::ensure_private_socket_parent(path)?;

        if let Err(error) = crate::ipc::socket_path::validate_socket_path(path) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsafe socket path after directory preparation: {error}"),
            ));
        }
        let listener = UnixListener::bind(path)?;
        ensure_socket_file(path)?;
        fs::secure_permissions(path, 0o600)?;
        Ok(Self {
            listener,
            hub,
            registry,
        })
    }

    /// Run the accept loop until `shutdown` fires.
    ///
    /// `lanes` is asked for a fresh sender per accepted connection rather than
    /// being handed one sender to clone, because who receives a session's
    /// commands is what decides the order they run in: see
    /// [`CommandLanes`].
    pub async fn run(self, lanes: impl CommandLanes, mut shutdown: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                result = self.listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let hub = Arc::clone(&self.hub);
                            let tx = lanes.open_lane();
                            let registry = self.registry.clone();
                            tokio::spawn(run_session(stream, hub, tx, registry));
                        }
                        Err(e) => {
                            error!("IPC accept error: {e}");
                            break;
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

async fn run_session(
    stream: UnixStream,
    hub: Arc<BroadcastHub>,
    command_tx: mpsc::Sender<CommandDispatch>,
    registry: ClientRegistry,
) {
    let _client_guard = registry.register();
    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let (write_tx, mut write_rx) = mpsc::channel::<String>(64);

    let write_task = tokio::spawn(async move {
        while let Some(msg) = write_rx.recv().await {
            if writer.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
        }
    });

    let mut subs = SessionSubscriptions::default();
    let mut state_rx = hub.subscribe_state();
    let mut events_rx = hub.subscribe_events();
    let mut logs_rx = hub.subscribe_logs();
    let mut frames = FrameReader::new(reader);

    loop {
        tokio::select! {
            // `FrameReader` owns the bytes of a frame that has not arrived in
            // full, which is what makes this branch safe to lose the race:
            // when a broadcast below wins, this future is dropped mid-frame
            // and the next iteration resumes the same frame.
            frame = frames.next_frame() => {
                match frame {
                    // The peer closed the connection between frames.
                    Ok(None) => break,
                    Ok(Some(frame)) => {
                        // A frame with nothing but whitespace in it is not a
                        // request; ignore it rather than end the session.
                        let trimmed = frame.trim_end_matches(['\n', '\r']);
                        if !trimmed.is_empty()
                            && handle_line(
                                trimmed.to_string(), &mut subs, &command_tx, &write_tx,
                            ).await == SessionControl::Close
                        {
                            break;
                        }
                    }
                    Err(FrameError::Oversized) => {
                        warn!("IPC frame exceeded max size ({MAX_IPC_LINE_BYTES} bytes), dropping client");
                        break;
                    }
                    Err(FrameError::MissingDelimiter) => {
                        warn!("IPC frame missing newline delimiter, dropping client");
                        break;
                    }
                    Err(FrameError::NotUtf8(e)) => {
                        debug!("IPC session read error: {e}");
                        break;
                    }
                    Err(FrameError::Io(e)) => {
                        debug!("IPC session read error: {e}");
                        break;
                    }
                }
            }

            res = state_rx.recv(), if subs.contains(Topic::State) => {
                match res {
                    Ok(msg) => send_encoded(&msg, &write_tx).await,
                    Err(RecvError::Lagged(n)) => warn!("state broadcast lagged {n}"),
                    Err(RecvError::Closed) => break,
                }
            }

            res = events_rx.recv(), if subs.contains(Topic::Events) => {
                match res {
                    Ok(msg) => send_encoded(&msg, &write_tx).await,
                    Err(RecvError::Lagged(n)) => warn!("events broadcast lagged {n}"),
                    Err(RecvError::Closed) => break,
                }
            }

            res = logs_rx.recv(), if subs.contains(Topic::Logs) => {
                match res {
                    Ok(msg) => send_encoded(&msg, &write_tx).await,
                    Err(RecvError::Lagged(n)) => warn!("logs broadcast lagged {n}"),
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    drop(write_tx);
    let _ = write_task.await;
}

async fn send_encoded(msg: &ServerMessage, write_tx: &mpsc::Sender<String>) {
    let encoded = match encode_framed(msg) {
        Ok(encoded) => encoded,
        Err(FrameEncodeError::Encode(error)) => {
            warn!("Failed to encode IPC message for client socket: {error}");
            return;
        }
        Err(FrameEncodeError::Oversized { len }) => {
            send_oversized_fallback(msg, len, write_tx).await;
            return;
        }
    };

    if write_tx.send(encoded).await.is_err() {
        warn!("Failed to queue IPC message for client socket");
    }
}

/// Deal with a message that will not fit in one frame.
///
/// A client waiting on a response still needs an answer, so the response is
/// replaced by a small protocol error carrying the same request id. A pushed
/// event has nobody waiting on it and is dropped instead.
async fn send_oversized_fallback(msg: &ServerMessage, len: usize, write_tx: &mpsc::Sender<String>) {
    let ServerMessage::Response { id, .. } = msg else {
        warn!("Dropping oversized IPC message ({len} > {MAX_IPC_LINE_BYTES} bytes)");
        return;
    };

    let fallback = ServerMessage::Response {
        id: id.clone(),
        ok: false,
        result: None,
        error: Some(ErrorPayload::new(
            PublicErrorCode::IpcProtocolError,
            "response frame too large",
        )),
    };

    match encode_framed(&fallback) {
        Ok(fallback_encoded) => {
            let _ = write_tx.send(fallback_encoded).await;
        }
        Err(FrameEncodeError::Oversized { len }) => {
            warn!("Dropping oversized fallback IPC response ({len} > {MAX_IPC_LINE_BYTES} bytes)")
        }
        Err(FrameEncodeError::Encode(error)) => {
            warn!("Failed to encode IPC fallback response: {error}")
        }
    }
}

async fn handle_line(
    line: String,
    subs: &mut SessionSubscriptions,
    command_tx: &mpsc::Sender<CommandDispatch>,
    write_tx: &mpsc::Sender<String>,
) -> SessionControl {
    let msg = match decode::<ClientMessage>(&line) {
        Ok(m) => m,
        Err(e) => {
            warn!("Malformed IPC message: {e}");
            return SessionControl::Close;
        }
    };

    match msg {
        ClientMessage::Command { id, command } => {
            handle_command(id, command, command_tx, write_tx).await
        }
        ClientMessage::Subscribe { id, topics } => {
            handle_subscribe(id, topics, subs, command_tx, write_tx).await
        }
    }
}

/// Tell the client its request was rejected.
///
/// Every rejection in this module also ends the session, so the `Close` that
/// stops the read loop is part of this helper rather than a line each caller
/// has to remember to write after it.
async fn reject(
    id: String,
    code: PublicErrorCode,
    message: &str,
    write_tx: &mpsc::Sender<String>,
) -> SessionControl {
    send_encoded(&err_response(id, code, message), write_tx).await;
    SessionControl::Close
}

/// Hand one command to the executor and answer the client when it replies.
async fn handle_command(
    id: String,
    command: CommandPayload,
    command_tx: &mpsc::Sender<CommandDispatch>,
    write_tx: &mpsc::Sender<String>,
) -> SessionControl {
    if let Err(error) = validate_ipc_request_id(&id) {
        return reject(id, PublicErrorCode::IpcProtocolError, &error, write_tx).await;
    }
    if let Err(error) = validate_command_name(&command.name) {
        return reject(id, PublicErrorCode::IpcProtocolError, &error, write_tx).await;
    }

    let write_tx = write_tx.clone();
    let id_clone = id.clone();
    let (reply_tx, reply_rx) = oneshot::channel();
    if command_tx
        .send(CommandDispatch {
            id,
            payload: command,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return reject(
            id_clone,
            PublicErrorCode::ServerError,
            "command handler unavailable",
            &write_tx,
        )
        .await;
    }

    // Waiting for the reply here would stop this session reading anything
    // else until the command finished, so the wait is spawned and the read
    // loop goes straight back to the socket.
    tokio::spawn(async move {
        let response = match reply_rx.await {
            Ok(r) => sanitize_response_id(r, &id_clone),
            Err(_) => err_response(
                id_clone,
                PublicErrorCode::ServerError,
                "command handler dropped",
            ),
        };
        send_encoded(&response, &write_tx).await;
    });

    SessionControl::Continue
}

/// Record what this session wants pushed to it, acknowledge the request, and —
/// for a first subscription to `state` — send the current snapshot straight
/// away so the client has something to draw before any change arrives.
async fn handle_subscribe(
    id: String,
    topics: Vec<String>,
    subs: &mut SessionSubscriptions,
    command_tx: &mpsc::Sender<CommandDispatch>,
    write_tx: &mpsc::Sender<String>,
) -> SessionControl {
    if let Err(error) = validate_ipc_request_id(&id) {
        return reject(id, PublicErrorCode::IpcProtocolError, &error, write_tx).await;
    }

    let topics = match normalize_subscribe_topics(topics) {
        Ok(topics) => topics,
        Err(error) => {
            send_encoded(&error.to_protocol_response(id), write_tx).await;
            return SessionControl::Close;
        }
    };

    // Checked before the inserts below: "first subscription to state" means
    // this session was not already subscribed when the request arrived.
    let needs_initial_state = topics.contains(&Topic::State) && !subs.contains(Topic::State);
    for topic in &topics {
        subs.insert(*topic);
    }

    let ack = ServerMessage::Response {
        id,
        ok: true,
        result: Some(serde_json::json!({ "subscribed": topics })),
        error: None,
    };
    send_encoded(&ack, write_tx).await;

    if needs_initial_state {
        send_initial_state(command_tx, write_tx).await;
    }

    SessionControl::Continue
}

/// Push the current snapshot to a session that has just subscribed to `state`.
async fn send_initial_state(
    command_tx: &mpsc::Sender<CommandDispatch>,
    write_tx: &mpsc::Sender<String>,
) {
    let write_tx = write_tx.clone();
    let (reply_tx, reply_rx) = oneshot::channel();
    let dispatch = CommandDispatch {
        id: STATE_INIT_REQUEST_ID.to_string(),
        payload: CommandPayload::simple(ServerCommand::GetSnapshot),
        reply: reply_tx,
    };
    if command_tx.send(dispatch).await.is_err() {
        return;
    }

    tokio::spawn(async move {
        // get_snapshot returns a Response; re-wrap the payload as a
        // state Event so next_event() on the client side accepts it.
        if let Ok(response) = reply_rx.await {
            match sanitize_response_id(response, STATE_INIT_REQUEST_ID) {
                ServerMessage::Response {
                    ok: true,
                    result: Some(data),
                    ..
                } => {
                    let state_event = ServerMessage::Event {
                        topic: Topic::State,
                        data,
                    };
                    send_encoded(&state_event, &write_tx).await;
                }
                _ => {
                    warn!("Ignoring malformed state-init response");
                }
            }
        }
    });
}

fn err_response(id: String, code: PublicErrorCode, message: &str) -> ServerMessage {
    ServerMessage::Response {
        id,
        ok: false,
        result: None,
        error: Some(ErrorPayload::new(code, message)),
    }
}

fn sanitize_response_id(response: ServerMessage, request_id: &str) -> ServerMessage {
    match response {
        ServerMessage::Response {
            id,
            ok,
            result,
            error,
        } => {
            if id == request_id && validate_ipc_request_id(&id).is_ok() {
                return ServerMessage::Response {
                    id,
                    ok,
                    result,
                    error,
                };
            }

            if id != request_id {
                warn!("Mismatched response id from command handler ({id}), expected {request_id}");
            } else {
                warn!(
                    "Malformed response id from command handler ({id}), replacing for request {request_id}"
                );
            }
            err_response(
                request_id.to_string(),
                PublicErrorCode::IpcProtocolError,
                if id == request_id {
                    "malformed response id"
                } else {
                    "response id mismatch"
                },
            )
        }
        _ => {
            warn!("Malformed command handler response type for request {request_id}");
            err_response(
                request_id.to_string(),
                PublicErrorCode::IpcProtocolError,
                "malformed command handler response",
            )
        }
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::{MAX_SUBSCRIBE_TOPICS, TOPIC_EVENTS, TOPIC_LOGS, TOPIC_STATE};
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[tokio::test]
    async fn bind_and_accept() {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");
        let hub = Arc::new(BroadcastHub::new());
        let server = IpcServer::bind(&socket_path, hub).unwrap();
        assert!(socket_path.exists());
        let metadata = std::fs::metadata(&socket_path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        drop(server);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bind_rejects_symlink_parent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        let linked_parent = dir.path().join("linked-parent");
        symlink(&parent, &linked_parent).unwrap();

        let socket_path = linked_parent.join("obsctl.sock");
        let hub = Arc::new(BroadcastHub::new());
        let err = IpcServer::bind(&socket_path, hub).err().unwrap();
        assert!(
            err.to_string().contains("unsafe socket path")
                || err.to_string().contains("No such file or directory")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bind_rejects_world_writable_parent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("private");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let socket_path = parent.join("obsctl.sock");

        let hub = Arc::new(BroadcastHub::new());
        let err = IpcServer::bind(&socket_path, hub).err().unwrap();
        assert!(
            err.to_string().contains("unsafe socket path")
                || err
                    .to_string()
                    .contains("unsafe socket path after directory preparation")
        );
    }

    #[tokio::test]
    async fn reject_empty_subscribe() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let mut subs = SessionSubscriptions::default();

        let handled = handle_line(
            serde_json::to_string(&ClientMessage::Subscribe {
                id: "sub-1".to_string(),
                topics: vec![],
            })
            .unwrap(),
            &mut subs,
            &command_tx,
            &write_tx,
        )
        .await;
        assert_eq!(handled, SessionControl::Close);

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        match msg {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "sub-1");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol response"),
        }
    }

    #[tokio::test]
    async fn reject_too_many_subscribe_topics() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let mut subs = SessionSubscriptions::default();

        let topics = (0..=MAX_SUBSCRIBE_TOPICS)
            .map(|idx| format!("topic-{idx}"))
            .collect::<Vec<_>>();
        let handled = handle_line(
            serde_json::to_string(&ClientMessage::Subscribe {
                id: "sub-2".to_string(),
                topics,
            })
            .unwrap(),
            &mut subs,
            &command_tx,
            &write_tx,
        )
        .await;
        assert_eq!(handled, SessionControl::Close);

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        match msg {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "sub-2");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol response"),
        }
    }

    #[tokio::test]
    async fn deduplicate_subscribe_topics_in_ack() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let mut subs = SessionSubscriptions::default();

        let handled = handle_line(
            serde_json::to_string(&ClientMessage::Subscribe {
                id: "sub-3".to_string(),
                topics: vec![
                    TOPIC_STATE.to_string(),
                    TOPIC_STATE.to_string(),
                    TOPIC_EVENTS.to_string(),
                    TOPIC_LOGS.to_string(),
                    TOPIC_EVENTS.to_string(),
                ],
            })
            .unwrap(),
            &mut subs,
            &command_tx,
            &write_tx,
        )
        .await;
        assert_eq!(handled, SessionControl::Continue);

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        let response_subscribed = match msg {
            ServerMessage::Response { id, ok, result, .. } => {
                assert_eq!(id, "sub-3");
                assert!(ok);
                result.expect("missing result")
            }
            _ => panic!("expected subscribe ack response"),
        };

        let subscribed = response_subscribed
            .get("subscribed")
            .and_then(|v| v.as_array())
            .expect("subscribed field missing");
        assert_eq!(
            subscribed
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![TOPIC_STATE, TOPIC_EVENTS, TOPIC_LOGS]
        );
        assert!(subs.contains(Topic::State));
        assert!(subs.contains(Topic::Events));
        assert!(subs.contains(Topic::Logs));
    }

    #[tokio::test]
    async fn oversized_response_message_uses_protocol_error_fallback() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);

        let huge = "x".repeat(MAX_IPC_LINE_BYTES);
        send_encoded(
            &ServerMessage::Response {
                id: "oversized-1".to_string(),
                ok: true,
                result: Some(serde_json::json!({ "data": huge })),
                error: None,
            },
            &write_tx,
        )
        .await;

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        match msg {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "oversized-1");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol response"),
        }
        assert!(resp.len() <= MAX_IPC_LINE_BYTES);
    }

    #[tokio::test]
    async fn oversized_event_is_dropped() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let huge = "x".repeat(MAX_IPC_LINE_BYTES);

        send_encoded(
            &ServerMessage::Event {
                topic: Topic::Logs,
                data: serde_json::json!({ "data": huge }),
            },
            &write_tx,
        )
        .await;

        assert!(write_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn reject_command_with_invalid_request_id() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let mut subs = SessionSubscriptions::default();

        let handled = handle_line(
            serde_json::to_string(&ClientMessage::Command {
                id: "has space".to_string(),
                command: CommandPayload {
                    name: "ping".to_string(),
                    args: serde_json::json!({}),
                },
            })
            .unwrap(),
            &mut subs,
            &command_tx,
            &write_tx,
        )
        .await;
        assert_eq!(handled, SessionControl::Close);

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        match msg {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "has space");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol response"),
        }
    }

    #[tokio::test]
    async fn reject_command_with_invalid_command_name() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let mut subs = SessionSubscriptions::default();

        let handled = handle_line(
            serde_json::to_string(&ClientMessage::Command {
                id: "cmd-1".to_string(),
                command: CommandPayload {
                    name: "has space".to_string(),
                    args: serde_json::json!({}),
                },
            })
            .unwrap(),
            &mut subs,
            &command_tx,
            &write_tx,
        )
        .await;
        assert_eq!(handled, SessionControl::Close);

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        match msg {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "cmd-1");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol response"),
        }
    }

    #[tokio::test]
    async fn replace_malformed_handler_response_id_with_protocol_error() {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(4);
        let (command_tx, mut command_rx) = mpsc::channel::<CommandDispatch>(4);
        let mut subs = SessionSubscriptions::default();

        let handled = handle_line(
            serde_json::to_string(&ClientMessage::Command {
                id: "cmd-1".to_string(),
                command: CommandPayload {
                    name: "ping".to_string(),
                    args: serde_json::json!({}),
                },
            })
            .unwrap(),
            &mut subs,
            &command_tx,
            &write_tx,
        )
        .await;
        assert_eq!(handled, SessionControl::Continue);

        let dispatch = command_rx.recv().await.expect("expected command dispatch");
        dispatch
            .reply
            .send(ServerMessage::Response {
                id: "bad id".to_string(),
                ok: true,
                result: Some(serde_json::json!({ "message": "pong" })),
                error: None,
            })
            .unwrap();

        let resp = write_rx.recv().await.unwrap();
        let msg = decode::<ServerMessage>(&resp).unwrap();
        match msg {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "cmd-1");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol response"),
        }
    }

    #[test]
    fn sanitize_response_id_requires_matching_request_id() {
        let response = sanitize_response_id(
            ServerMessage::Response {
                id: "other-id".to_string(),
                ok: true,
                result: Some(serde_json::json!({ "message": "ok" })),
                error: None,
            },
            "expected-id",
        );

        match response {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "expected-id");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol error response"),
        }
    }

    #[test]
    fn sanitize_response_id_rejects_non_response_message() {
        let response = sanitize_response_id(
            ServerMessage::Event {
                topic: Topic::Logs,
                data: serde_json::json!({ "message": "not a response" }),
            },
            "expected-id",
        );

        match response {
            ServerMessage::Response { id, ok, error, .. } => {
                assert_eq!(id, "expected-id");
                assert!(!ok);
                assert_eq!(
                    error.unwrap().code,
                    PublicErrorCode::IpcProtocolError.as_str().to_string()
                );
            }
            _ => panic!("expected protocol error response"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn drop_client_frame_without_newline_delimiter() {
        use crate::ipc::protocol::{ClientMessage, CommandPayload};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;
        use tokio::sync::watch;
        use tokio::time::{Duration, timeout};

        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("server.sock");
        let hub = Arc::new(BroadcastHub::new());
        let server = IpcServer::bind(&socket_path, hub).unwrap();
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let server_handle = tokio::spawn(server.run(command_tx, shutdown_rx));

        let mut stream = UnixStream::connect(&socket_path).await.unwrap();
        let malformed = serde_json::to_string(&ClientMessage::Command {
            id: "missing-newline".to_string(),
            command: CommandPayload {
                name: "ping".to_string(),
                args: serde_json::json!({}),
            },
        })
        .unwrap();
        stream.write_all(malformed.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();

        let mut buf = [0u8; 1];
        let n = timeout(Duration::from_millis(200), stream.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, 0);

        shutdown_tx.send(true).unwrap();
        server_handle.await.unwrap();
    }
    /// A frame split across two writes, with a broadcast arriving in between,
    /// must still be understood.
    ///
    /// This is the cancellation case. The session's read used to be
    /// `read_line` inside a `select!`; when the logs branch wins the race
    /// mid-frame the read future is dropped, and `read_line` is documented as
    /// discarding what it had already read. The first half of the command was
    /// therefore lost and the second half parsed as a whole frame — which is
    /// not valid JSON, so the client was dropped. Publishing a log between the
    /// two writes is what forces that race deterministically.
    #[tokio::test]
    async fn command_split_around_a_broadcast_is_reassembled() {
        use crate::ipc::protocol::{ClientMessage, CommandPayload, LogEvent, LogLevel};
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt, BufReader as TokioBufReader};
        use tokio::net::UnixStream;
        use tokio::sync::watch;
        use tokio::time::{Duration, timeout};

        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("server.sock");
        let hub = Arc::new(BroadcastHub::new());
        let server = IpcServer::bind(&socket_path, Arc::clone(&hub)).unwrap();
        let (command_tx, mut command_rx) = mpsc::channel::<CommandDispatch>(4);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(server.run(command_tx, shutdown_rx));

        tokio::spawn(async move {
            while let Some(dispatch) = command_rx.recv().await {
                let _ = dispatch.reply.send(ServerMessage::Response {
                    id: dispatch.id,
                    ok: true,
                    result: Some(serde_json::json!({ "message": "pong" })),
                    error: None,
                });
            }
        });

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = TokioBufReader::new(read_half).lines();

        // Subscribe to logs so the broadcast branch of the session's `select!`
        // is armed and can win the race against the half-read frame.
        let subscribe = serde_json::to_string(&ClientMessage::Subscribe {
            id: "sub".to_string(),
            topics: vec![TOPIC_LOGS.to_string()],
        })
        .unwrap();
        write_half
            .write_all(format!("{subscribe}\n").as_bytes())
            .await
            .unwrap();
        let ack = timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(ack.contains("subscribed"), "unexpected ack: {ack}");

        let frame = format!(
            "{}\n",
            serde_json::to_string(&ClientMessage::Command {
                id: "split-frame".to_string(),
                command: CommandPayload::simple(ServerCommand::Ping),
            })
            .unwrap()
        );
        let (head, tail) = frame.split_at(frame.len() / 2);

        write_half.write_all(head.as_bytes()).await.unwrap();
        write_half.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel the in-flight read.
        hub.publish_log(LogEvent::new(LogLevel::Info, "interleaved"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        write_half.write_all(tail.as_bytes()).await.unwrap();
        write_half.flush().await.unwrap();

        // The log event arrives first; the command reply must still follow.
        let mut reply = None;
        for _ in 0..4 {
            let line = timeout(Duration::from_secs(2), lines.next_line())
                .await
                .expect("session went quiet after a split frame")
                .unwrap();
            match line {
                Some(line) if line.contains("split-frame") => {
                    reply = Some(line);
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(
            reply.is_some(),
            "the half of the frame read before the broadcast was lost"
        );

        shutdown_tx.send(true).unwrap();
        server_handle.await.unwrap();
    }

    /// An unterminated frame must be cut off at the size cap rather than
    /// allowed to grow the session's buffer without bound.
    #[tokio::test]
    async fn oversized_unterminated_frame_drops_the_client() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;
        use tokio::sync::watch;
        use tokio::time::{Duration, timeout};

        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("server.sock");
        let hub = Arc::new(BroadcastHub::new());
        let server = IpcServer::bind(&socket_path, hub).unwrap();
        let (command_tx, _command_rx) = mpsc::channel::<CommandDispatch>(4);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(server.run(command_tx, shutdown_rx));

        let mut stream = UnixStream::connect(&socket_path).await.unwrap();
        // No newline anywhere, comfortably past the cap.
        let flood = vec![b'a'; MAX_IPC_LINE_BYTES * 2];
        // The peer may be dropped mid-write once the server gives up, which is
        // the behaviour under test rather than a failure.
        let _ = stream.write_all(&flood).await;

        let mut buf = [0u8; 1];
        let n = timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .expect("server never dropped an oversized frame")
            .unwrap_or(0);
        assert_eq!(n, 0, "expected the session to close");

        shutdown_tx.send(true).unwrap();
        server_handle.await.unwrap();
    }
}
