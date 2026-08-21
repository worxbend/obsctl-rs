use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{
    io::{AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::{
    domain::{errors::ObsctlError, result::Result},
    ipc::socket_path::ensure_socket_file,
    ipc::socket_path::validate_socket_path,
    ipc::{
        codec::{FrameEncodeError, FrameError, FrameReader, decode, encode_framed},
        protocol::{
            ClientMessage, CommandPayload, ServerMessage, normalize_subscribe_topics,
            validate_command_name, validate_ipc_request_id,
        },
    },
};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

/// How many frames this client will skip while waiting for one specific
/// reply before giving up.
///
/// This is one peer's own patience, not part of the protocol: the server
/// never learns the number and nothing on the wire depends on it, so it lives
/// here next to the code that spends the budget.
const MAX_UNMATCHED_IPC_RESPONSES: usize = 32;

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

/// How long a client waits for the daemon's answer before giving up.
///
/// The daemon can legitimately take a while: `dump-config` performs two OBS
/// round trips (2500 ms each by default) plus a config-file write and reload,
/// so a short budget here would abort healthy commands. Thirty seconds is far
/// above anything the daemon does on purpose and still bounded, which is the
/// whole point — `obsctl` is built to be scripted from cron jobs, hotkeys and
/// stream-deck buttons, and a command that never returns gives its caller no
/// exit code to react to and no way out but an external kill.
pub const IPC_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Run one request/response round trip under [`IPC_RESPONSE_TIMEOUT`].
///
/// The timeout lives here, around the whole exchange, rather than around the
/// individual reads inside it: what a caller cares about is "did this command
/// finish", not which particular read was outstanding when the daemon wedged.
pub async fn send_command_within_timeout(
    client: &mut IpcClient,
    payload: CommandPayload,
) -> Result<ServerMessage> {
    match tokio::time::timeout(IPC_RESPONSE_TIMEOUT, client.send_command(payload)).await {
        Ok(result) => result,
        Err(_) => Err(ObsctlError::IpcTimeout {
            seconds: IPC_RESPONSE_TIMEOUT.as_secs(),
        }),
    }
}

pub struct IpcClient {
    frames: FrameReader<BufReader<tokio::net::unix::OwnedReadHalf>>,
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
            frames: FrameReader::new(BufReader::new(reader)),
            writer,
        })
    }

    /// Read one frame, saying what went wrong in the caller's terms.
    ///
    /// The framing itself is `FrameReader`'s job; what this adds is the
    /// vocabulary the rest of the client speaks. `frame_name` is what the
    /// caller was waiting for ("response", "event"), and `closed_error`
    /// describes the peer hanging up at that particular point, which is not a
    /// framing fault but still leaves the caller with no answer.
    async fn read_frame(
        &mut self,
        closed_error: &'static str,
        frame_name: &'static str,
    ) -> Result<String> {
        match self.frames.next_frame().await {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => Err(ObsctlError::IpcProtocolError(closed_error.to_string())),
            Err(FrameError::Oversized) => Err(ObsctlError::IpcProtocolError(format!(
                "{frame_name} frame too large",
            ))),
            Err(FrameError::MissingDelimiter) => Err(ObsctlError::IpcProtocolError(format!(
                "{frame_name} frame missing newline delimiter",
            ))),
            Err(FrameError::NotUtf8(_)) => Err(ObsctlError::IpcProtocolError(format!(
                "{frame_name} frame is not valid utf-8"
            ))),
            // A socket that fails part-way through a frame has left behind a
            // frame that can never be completed; one that fails before any of
            // it arrived is the peer hanging up between frames.
            Err(FrameError::Io(_)) if self.frames.has_partial_frame() => {
                Err(ObsctlError::IpcProtocolError(format!(
                    "{frame_name} frame missing newline delimiter"
                )))
            }
            Err(FrameError::Io(_)) => Err(ObsctlError::IpcProtocolError(closed_error.to_string())),
        }
    }

    /// Encode one client message and write it to the socket.
    ///
    /// `encode_framed` applies the size limit shared with the server, which
    /// refuses to read an over-long line: sending one would mean a dropped
    /// connection rather than an error the caller can report. Both senders
    /// below go through here so neither can skip it.
    async fn send_frame(&mut self, msg: &ClientMessage) -> Result<()> {
        let encoded = encode_framed(msg).map_err(|error| match error {
            FrameEncodeError::Encode(error) => error,
            FrameEncodeError::Oversized { .. } => {
                ObsctlError::IpcProtocolError("request frame too large".to_string())
            }
        })?;
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
            // Back to strings: the topics were validated as `Topic` values,
            // but the wire field is a list of names and stays one.
            topics: topics
                .into_iter()
                .map(|topic| topic.as_str().to_string())
                .collect(),
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

    /// The regression test for a daemon that accepts the connection and then
    /// says nothing. Before the timeout existed, this call never returned:
    /// the response read looped until the process was killed from outside,
    /// which for a scripted `obsctl mute Mic` meant no exit code and no output.
    ///
    /// `start_paused` makes tokio advance its clock as soon as every task is
    /// idle, so the thirty-second budget elapses instantly and the test asserts
    /// on the outcome rather than on real elapsed time. There is no sleep and
    /// nothing to race.
    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn send_command_gives_up_when_the_daemon_never_replies() {
        use crate::domain::errors::ObsctlError;
        use crate::ipc::protocol::CommandPayload;
        use tokio::net::UnixListener;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("silent.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();

        // Accept the connection and hold it open without ever answering, which
        // is what a wedged daemon looks like from the client's side.
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
            drop(stream);
        });

        let mut client = IpcClient::connect(&socket_path).await.unwrap();
        let err = super::send_command_within_timeout(
            &mut client,
            CommandPayload {
                name: "ping".to_string(),
                args: serde_json::Value::Null,
            },
        )
        .await
        .unwrap_err();

        match err {
            ObsctlError::IpcTimeout { seconds } => {
                assert_eq!(seconds, super::IPC_RESPONSE_TIMEOUT.as_secs());
            }
            other => panic!("expected a timeout, got {other:?}"),
        }

        server_handle.abort();
    }

    /// A timeout has to reach a script as a distinct, non-zero exit code, or
    /// the caller cannot tell "the daemon is wedged" from "the command failed".
    #[test]
    fn the_timeout_error_maps_to_the_ipc_exit_code() {
        use crate::domain::errors::ObsctlError;
        use crate::ipc::protocol::{PublicErrorCode, public_error_code};

        let error = ObsctlError::IpcTimeout { seconds: 30 };
        assert_eq!(error.exit_code(), 6);

        let code = public_error_code(&error);
        assert_eq!(code, PublicErrorCode::IpcTimeout);
        assert_eq!(code.as_str(), "IPC_TIMEOUT");
        assert_eq!(code.exit_code(), 6);
    }
}
