use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::{
    domain::{errors::ObsctlError, result::Result},
    ipc::socket_path::ensure_socket_file,
    ipc::socket_path::validate_socket_path,
    ipc::{
        codec::{decode, encode},
        protocol::{
            ClientMessage, CommandPayload, MAX_IPC_LINE_BYTES, MAX_UNMATCHED_IPC_RESPONSES,
            ServerMessage, normalize_subscribe_topics, validate_command_name,
            validate_ipc_request_id,
        },
    },
};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A budget for frames that arrive while waiting for one specific reply.
///
/// The server may legitimately interleave pushed events and replies to other
/// requests, so a frame that is not the one being waited on is skipped rather
/// than treated as an error. Without a cap that skipping is an unbounded loop,
/// which is why the count exists at all.
struct UnmatchedFrames {
    count: usize,
    waiting_for: &'static str,
}

impl UnmatchedFrames {
    fn waiting_for(waiting_for: &'static str) -> Self {
        Self {
            count: 0,
            waiting_for,
        }
    }

    /// Count one skipped frame, failing once the cap is passed.
    fn record(&mut self) -> Result<()> {
        self.count += 1;
        if self.count > MAX_UNMATCHED_IPC_RESPONSES {
            return Err(ObsctlError::IpcProtocolError(format!(
                "too many unmatched IPC frames waiting for {}",
                self.waiting_for
            )));
        }
        Ok(())
    }
}

pub fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{n:06}")
}

pub struct IpcClient {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl IpcClient {
    pub async fn connect(path: &Path) -> Result<Self> {
        if let Err(error) = validate_socket_path(path) {
            return Err(ObsctlError::IpcConnectionFailed(format!(
                "unsafe socket path: {error}"
            )));
        }

        if let Err(error) = ensure_socket_file(path) {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                return Err(ObsctlError::IpcConnectionFailed(
                    "socket path is not a Unix socket".to_string(),
                ));
            } else if error.kind() != std::io::ErrorKind::NotFound {
                return Err(ObsctlError::IpcConnectionFailed(error.to_string()));
            }
        }

        let stream = UnixStream::connect(path)
            .await
            .map_err(|e| ObsctlError::IpcConnectionFailed(e.to_string()))?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    async fn read_frame(
        &mut self,
        closed_error: &'static str,
        frame_name: &'static str,
    ) -> Result<String> {
        let mut line = Vec::<u8>::new();
        loop {
            let byte = match self.reader.read_u8().await {
                Ok(byte) => byte,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    continue;
                }
                Err(_) => {
                    if line.is_empty() {
                        return Err(ObsctlError::IpcProtocolError(closed_error.to_string()));
                    }
                    return Err(ObsctlError::IpcProtocolError(format!(
                        "{frame_name} frame missing newline delimiter",
                    )));
                }
            };

            line.push(byte);
            if line.len() > MAX_IPC_LINE_BYTES {
                return Err(ObsctlError::IpcProtocolError(format!(
                    "{frame_name} frame too large",
                )));
            }
            if byte == b'\n' {
                break;
            }
        }

        let line = String::from_utf8(line).map_err(|_| {
            ObsctlError::IpcProtocolError(format!("{frame_name} frame is not valid utf-8"))
        })?;
        if !line.ends_with('\n') {
            return Err(ObsctlError::IpcProtocolError(format!(
                "{frame_name} frame missing newline delimiter",
            )));
        }
        Ok(line)
    }

    /// Encode one client message and write it to the socket.
    ///
    /// The size check is the client's half of the framing contract: the server
    /// refuses to read a line longer than `MAX_IPC_LINE_BYTES`, so sending one
    /// would mean a dropped connection rather than an error the caller can
    /// report. Both senders below go through here so neither can skip it.
    async fn send_frame(&mut self, msg: &ClientMessage) -> Result<()> {
        let encoded = encode(msg)?;
        if encoded.len() > MAX_IPC_LINE_BYTES {
            return Err(ObsctlError::IpcProtocolError(
                "request frame too large".to_string(),
            ));
        }
        self.writer
            .write_all(encoded.as_bytes())
            .await
            .map_err(ObsctlError::Io)
    }

    /// Send a command and wait for the correlated response.
    pub async fn send_command(&mut self, payload: CommandPayload) -> Result<ServerMessage> {
        validate_command_name(&payload.name).map_err(|error| {
            ObsctlError::IpcProtocolError(format!("invalid command name: {error}"))
        })?;

        let id = next_request_id();
        let msg = ClientMessage::Command {
            id: id.clone(),
            command: payload,
        };
        self.send_frame(&msg).await?;

        let mut unmatched = UnmatchedFrames::waiting_for("response");
        loop {
            let frame = self.read_frame("connection closed", "response").await?;
            let msg = decode::<ServerMessage>(&frame).map_err(|e| {
                ObsctlError::IpcProtocolError(format!("malformed response frame: {e}"))
            })?;
            match msg {
                ServerMessage::Response {
                    id: ref resp_id, ..
                } => {
                    validate_ipc_request_id(resp_id).map_err(|error| {
                        ObsctlError::IpcProtocolError(format!("malformed response id: {error}"))
                    })?;
                    if resp_id == &id {
                        return Ok(msg);
                    }
                    unmatched.record()?;
                }
                _ => {
                    unmatched.record()?;
                    continue;
                }
            }
        }
    }

    /// Subscribe to the given topics, returning once the server acks.
    pub async fn subscribe(&mut self, topics: &[&str]) -> Result<()> {
        let topics = match normalize_subscribe_topics(topics) {
            Ok(topics) => topics,
            Err(error) => {
                return Err(ObsctlError::IpcProtocolError(
                    error.as_protocol_error_message(),
                ));
            }
        };

        let id = next_request_id();
        let msg = ClientMessage::Subscribe {
            id: id.clone(),
            topics,
        };
        self.send_frame(&msg).await?;

        let mut unmatched = UnmatchedFrames::waiting_for("subscribe response");
        loop {
            let frame = self
                .read_frame("connection closed before subscribe ack", "response")
                .await?;
            let msg = decode::<ServerMessage>(&frame).map_err(|e| {
                ObsctlError::IpcProtocolError(format!("malformed response frame: {e}"))
            })?;
            match msg {
                ServerMessage::Response {
                    id: resp_id,
                    ok,
                    error,
                    ..
                } => {
                    validate_ipc_request_id(&resp_id).map_err(|error| {
                        ObsctlError::IpcProtocolError(format!("malformed response id: {error}"))
                    })?;
                    if resp_id == id {
                        if ok {
                            return Ok(());
                        } else {
                            let msg = error
                                .as_ref()
                                .map(|e| e.message.as_str())
                                .unwrap_or("subscribe rejected");
                            return Err(ObsctlError::IpcProtocolError(msg.to_string()));
                        }
                    }
                    unmatched.record()?;
                }
                _ => {
                    unmatched.record()?;
                }
            }
        }
    }

    /// Read the next pushed event from the server, skipping responses.
    pub async fn next_event(&mut self) -> Result<ServerMessage> {
        let mut unmatched = UnmatchedFrames::waiting_for("event");
        loop {
            let frame = self.read_frame("connection closed", "event").await?;
            let msg = decode::<ServerMessage>(&frame).map_err(|e| {
                ObsctlError::IpcProtocolError(format!("malformed event frame: {e}"))
            })?;
            match msg {
                msg @ ServerMessage::Event { .. } => return Ok(msg),
                ServerMessage::Response { id, .. } => {
                    validate_ipc_request_id(&id).map_err(|error| {
                        ObsctlError::IpcProtocolError(format!("malformed response id: {error}"))
                    })?;
                    unmatched.record()?;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{IpcClient, MAX_UNMATCHED_IPC_RESPONSES};
    use std::path::Path;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[tokio::test]
    async fn connect_rejects_relative_socket_path() {
        let result = IpcClient::connect(Path::new("relative.sock")).await;
        assert!(result.is_err());
        let msg = format!("{:?}", result.err().unwrap());
        assert!(msg.contains("unsafe socket path"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_rejects_symlinked_socket_path() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real.sock");
        let link = dir.path().join("link.sock");
        symlink(&real, &link).unwrap();

        let result = IpcClient::connect(&link).await;
        assert!(result.is_err());
        let msg = format!("{:?}", result.err().unwrap());
        assert!(msg.contains("unsafe socket path"));
    }

    #[tokio::test]
    async fn connect_rejects_regular_file_path() {
        use std::fs::File;
        use std::io::Write;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("not-socket.txt");
        let mut file = File::create(&socket_path).unwrap();
        file.write_all(b"not a socket").unwrap();

        let result = IpcClient::connect(&socket_path).await;
        assert!(result.is_err());
        let msg = format!("{:?}", result.err().unwrap());
        assert!(msg.contains("socket path is not a Unix socket"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn send_command_rejects_malformed_response_id() {
        use crate::domain::errors::ObsctlError;
        use crate::ipc::protocol::{CommandPayload, ServerMessage};
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let response = ServerMessage::Response {
                id: "bad id".to_string(),
                ok: true,
                result: None,
                error: None,
            };
            let frame = crate::ipc::codec::encode(&response).unwrap();
            stream.write_all(frame.as_bytes()).await.unwrap();
        });

        let mut client = IpcClient::connect(&socket_path).await.unwrap();
        let err = client
            .send_command(CommandPayload {
                name: "ping".to_string(),
                args: serde_json::Value::Null,
            })
            .await
            .unwrap_err();
        let msg = format!("{:?}", err);
        assert!(msg.contains("malformed response id"), "{msg}");
        match err {
            ObsctlError::IpcProtocolError(message) => {
                assert!(message.contains("malformed response id"));
            }
            _ => panic!("expected protocol error"),
        }

        server_handle.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn send_command_rejects_malformed_response_without_newline() {
        use crate::domain::errors::ObsctlError;
        use crate::ipc::protocol::{CommandPayload, ServerMessage};
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let response = ServerMessage::Response {
                id: "req-000001".to_string(),
                ok: true,
                result: Some(serde_json::json!({ "message": "pong" })),
                error: None,
            };
            let frame = crate::ipc::codec::encode(&response).unwrap();
            let framed = frame.trim_end_matches('\n');
            stream.write_all(framed.as_bytes()).await.unwrap();
        });

        let mut client = IpcClient::connect(&socket_path).await.unwrap();
        let err = client
            .send_command(CommandPayload {
                name: "ping".to_string(),
                args: serde_json::Value::Null,
            })
            .await
            .unwrap_err();
        let msg = format!("{:?}", err);
        assert!(msg.contains("frame missing newline delimiter"), "{msg}");
        match err {
            ObsctlError::IpcProtocolError(message) => {
                assert!(message.contains("frame missing newline delimiter"));
            }
            _ => panic!("expected protocol error"),
        }

        server_handle.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn next_event_rejects_unbounded_non_event_frames() {
        use crate::ipc::{codec::encode, protocol::ServerMessage};
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        let server_handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let payload = ServerMessage::Response {
                id: "id".to_string(),
                ok: true,
                result: None,
                error: None,
            };
            let frame = encode(&payload).unwrap();
            for _ in 0..=(MAX_UNMATCHED_IPC_RESPONSES + 1) {
                stream
                    .write_all(frame.as_bytes())
                    .await
                    .expect("server write should succeed");
            }
        });

        let mut client = IpcClient::connect(&socket_path).await.unwrap();
        let err = client.next_event().await.unwrap_err();
        let msg = format!("{:?}", err);
        assert!(
            msg.contains("too many unmatched IPC frames waiting for event"),
            "{msg}"
        );

        server_handle.await.unwrap();
    }
}
