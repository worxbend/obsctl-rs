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
        ClientMessage, CommandPayload, ErrorPayload, PublicErrorCode, ServerMessage, TOPIC_EVENTS,
        TOPIC_LOGS, TOPIC_STATE, is_valid_topic,
    },
    session::{BroadcastHub, CommandDispatch, SessionSubscriptions},
};

pub struct IpcServer {
    listener: UnixListener,
    hub: Arc<BroadcastHub>,
}

impl IpcServer {
    pub fn bind(path: &Path, hub: Arc<BroadcastHub>) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self { listener, hub })
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
                            tokio::spawn(run_session(stream, hub, tx));
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
) {
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
                        let trimmed = line_buf.trim().to_string();
                        line_buf.clear();
                        if !trimmed.is_empty() {
                            handle_line(trimmed, &mut subs, &command_tx, &write_tx).await;
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
    if let Ok(encoded) = encode(msg) {
        let _ = write_tx.send(encoded).await;
    }
}

async fn handle_line(
    line: String,
    subs: &mut SessionSubscriptions,
    command_tx: &mpsc::Sender<CommandDispatch>,
    write_tx: &mpsc::Sender<String>,
) {
    let msg = match decode::<ClientMessage>(&line) {
        Ok(m) => m,
        Err(e) => {
            warn!("Malformed IPC message: {e}");
            return;
        }
    };

    match msg {
        ClientMessage::Command { id, command } => {
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
                return;
            }
            tokio::spawn(async move {
                let response = match reply_rx.await {
                    Ok(r) => r,
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
            let invalid: Vec<&String> = topics.iter().filter(|t| !is_valid_topic(t)).collect();
            if !invalid.is_empty() {
                let names = invalid
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                send_encoded(
                    &err_response(
                        id,
                        PublicErrorCode::IpcProtocolError,
                        &format!("unknown topics: {names}"),
                    ),
                    write_tx,
                )
                .await;
                return;
            }

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
                    id: format!("{id}-init"),
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
                        if let Ok(ServerMessage::Response {
                            ok: true,
                            result: Some(data),
                            ..
                        }) = reply_rx.await
                        {
                            let state_event = ServerMessage::Event {
                                topic: TOPIC_STATE.to_string(),
                                data,
                            };
                            send_encoded(&state_event, &write_tx).await;
                        }
                    });
                }
            }
        }
    }
}

fn err_response(id: String, code: PublicErrorCode, message: &str) -> ServerMessage {
    ServerMessage::Response {
        id,
        ok: false,
        result: None,
        error: Some(ErrorPayload::new(code, message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn bind_and_accept() {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");
        let hub = Arc::new(BroadcastHub::new());
        let server = IpcServer::bind(&socket_path, hub).unwrap();
        assert!(socket_path.exists());
        drop(server);
    }
}
