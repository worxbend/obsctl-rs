use std::{path::Path, sync::Arc};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{broadcast::error::RecvError, mpsc, oneshot, watch},
};
use tracing::{debug, error, warn};

use crate::ipc::{
    codec::{decode, encode},
    protocol::{
        ClientMessage, CommandPayload, ErrorPayload, MAX_IPC_LINE_BYTES, PublicErrorCode,
        ServerMessage, TOPIC_EVENTS, TOPIC_LOGS, TOPIC_STATE, Topic, normalize_subscribe_topics,
        validate_command_name, validate_ipc_request_id,
    },
    session::{BroadcastHub, CommandDispatch, SessionSubscriptions},
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
    pub async fn run(
        self,
        command_tx: mpsc::Sender<CommandDispatch>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                result = self.listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let hub = Arc::clone(&self.hub);
                            let tx = command_tx.clone();
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
    let mut reader = BufReader::new(reader);
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
    let mut line_buf = String::new();

    loop {
        tokio::select! {
            n = reader.read_line(&mut line_buf) => {
                match n {
                    Ok(0) => break,
                    Ok(_) => {
                        if line_buf.len() > MAX_IPC_LINE_BYTES {
                            warn!(
                                "IPC frame exceeded max size ({MAX_IPC_LINE_BYTES} bytes), dropping client"
                            );
                            break;
                        }
                        if !line_buf.ends_with('\n') {
                            warn!("IPC frame missing newline delimiter, dropping client");
                            break;
                        }
                        let trimmed = line_buf.trim_end_matches(['\n', '\r']).to_string();
                        line_buf.clear();
                        if !trimmed.is_empty()
                            && !handle_line(trimmed, &mut subs, &command_tx, &write_tx).await
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        debug!("IPC session read error: {e}");
                        break;
                    }
                }
            }

            res = state_rx.recv(), if subs.contains(TOPIC_STATE) => {
                match res {
                    Ok(msg) => send_encoded(&msg, &write_tx).await,
                    Err(RecvError::Lagged(n)) => warn!("state broadcast lagged {n}"),
                    Err(RecvError::Closed) => break,
                }
            }

            res = events_rx.recv(), if subs.contains(TOPIC_EVENTS) => {
                match res {
                    Ok(msg) => send_encoded(&msg, &write_tx).await,
                    Err(RecvError::Lagged(n)) => warn!("events broadcast lagged {n}"),
                    Err(RecvError::Closed) => break,
                }
            }

            res = logs_rx.recv(), if subs.contains(TOPIC_LOGS) => {
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
    let encoded = match encode(msg) {
        Ok(encoded) => encoded,
        Err(error) => {
            warn!("Failed to encode IPC message for client socket: {error}");
            return;
        }
    };

    if encoded.len() > MAX_IPC_LINE_BYTES {
        if let ServerMessage::Response { id, .. } = msg {
            let fallback = ServerMessage::Response {
                id: id.clone(),
                ok: false,
                result: None,
                error: Some(ErrorPayload::new(
                    PublicErrorCode::IpcProtocolError,
                    "response frame too large",
                )),
            };
            match encode(&fallback) {
                Ok(fallback_encoded) if fallback_encoded.len() <= MAX_IPC_LINE_BYTES => {
                    let _ = write_tx.send(fallback_encoded).await;
                }
                Ok(fallback_encoded) => warn!(
                    "Dropping oversized fallback IPC response ({len} > {MAX_IPC_LINE_BYTES} bytes)",
                    len = fallback_encoded.len()
                ),
                Err(error) => warn!("Failed to encode IPC fallback response: {error}"),
            };
        } else {
            warn!(
                "Dropping oversized IPC message ({len} > {MAX_IPC_LINE_BYTES} bytes)",
                len = encoded.len()
            );
        }
        return;
    }

    if write_tx.send(encoded).await.is_err() {
        warn!("Failed to queue IPC message for client socket");
    }
}

async fn handle_line(
    line: String,
    subs: &mut SessionSubscriptions,
    command_tx: &mpsc::Sender<CommandDispatch>,
    write_tx: &mpsc::Sender<String>,
) -> bool {
    let msg = match decode::<ClientMessage>(&line) {
        Ok(m) => m,
        Err(e) => {
            warn!("Malformed IPC message: {e}");
            return false;
        }
    };

    match msg {
        ClientMessage::Command { id, command } => {
            if let Err(error) = validate_ipc_request_id(&id) {
                send_encoded(
                    &err_response(id, PublicErrorCode::IpcProtocolError, &error),
                    write_tx,
                )
                .await;
                return false;
            }
            if let Err(error) = validate_command_name(&command.name) {
                send_encoded(
                    &err_response(id, PublicErrorCode::IpcProtocolError, &error),
                    write_tx,
                )
                .await;
                return false;
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
                send_encoded(
                    &err_response(
                        id_clone,
                        PublicErrorCode::ServerError,
                        "command handler unavailable",
                    ),
                    &write_tx,
                )
                .await;
                return false;
            }
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
        }

        ClientMessage::Subscribe { id, topics } => {
            if let Err(error) = validate_ipc_request_id(&id) {
                send_encoded(
                    &err_response(id, PublicErrorCode::IpcProtocolError, &error),
                    write_tx,
                )
                .await;
                return false;
            }

            let topics = match normalize_subscribe_topics(topics) {
                Ok(topics) => topics,
                Err(error) => {
                    send_encoded(&error.to_protocol_response(id), write_tx).await;
                    return false;
                }
            };
            let needs_initial_state =
                topics.iter().any(|t| t == TOPIC_STATE) && !subs.is_state_subscribed();
            for t in &topics {
                subs.insert(t.clone());
            }

            let ack = ServerMessage::Response {
                id: id.clone(),
                ok: true,
                result: Some(serde_json::json!({ "subscribed": topics })),
                error: None,
            };
            send_encoded(&ack, write_tx).await;

            if needs_initial_state {
                let write_tx = write_tx.clone();
                let (reply_tx, reply_rx) = oneshot::channel();
                let dispatch = CommandDispatch {
                    id: STATE_INIT_REQUEST_ID.to_string(),
                    payload: CommandPayload {
                        name: "get_snapshot".to_string(),
                        args: serde_json::Value::Null,
                    },
                    reply: reply_tx,
                };
                if command_tx.send(dispatch).await.is_ok() {
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
            }
        }
    }

    true
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
    use crate::ipc::protocol::MAX_SUBSCRIBE_TOPICS;
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
        assert!(!handled);

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
        assert!(!handled);

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
        assert!(handled);

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
        assert!(subs.contains(TOPIC_STATE));
        assert!(subs.contains(TOPIC_EVENTS));
        assert!(subs.contains(TOPIC_LOGS));
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
        assert!(!handled);

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
        assert!(!handled);

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
        assert!(handled);

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
}
