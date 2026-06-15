use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

use crate::{
    domain::{errors::ObsctlError, result::Result},
    ipc::{
        codec::{decode, encode},
        protocol::{ClientMessage, CommandPayload, ServerMessage},
    },
};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

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
        let stream = UnixStream::connect(path)
            .await
            .map_err(|e| ObsctlError::IpcConnectionFailed(e.to_string()))?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    /// Send a command and wait for the correlated response.
    pub async fn send_command(&mut self, payload: CommandPayload) -> Result<ServerMessage> {
        let id = next_request_id();
        let msg = ClientMessage::Command {
            id: id.clone(),
            command: payload,
        };
        let encoded = encode(&msg)?;
        self.writer
            .write_all(encoded.as_bytes())
            .await
            .map_err(ObsctlError::Io)?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(ObsctlError::Io)?;
            if n == 0 {
                return Err(ObsctlError::IpcProtocolError(
                    "connection closed".to_string(),
                ));
            }
            if let Ok(msg @ ServerMessage::Response { .. }) = decode::<ServerMessage>(line.trim())
                && let ServerMessage::Response {
                    id: ref resp_id, ..
                } = msg
                && resp_id == &id
            {
                return Ok(msg);
            }
        }
    }

    /// Subscribe to the given topics, returning once the server acks.
    pub async fn subscribe(&mut self, topics: &[&str]) -> Result<()> {
        let id = next_request_id();
        let msg = ClientMessage::Subscribe {
            id: id.clone(),
            topics: topics.iter().map(|&s| s.to_string()).collect(),
        };
        let encoded = encode(&msg)?;
        self.writer
            .write_all(encoded.as_bytes())
            .await
            .map_err(ObsctlError::Io)?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(ObsctlError::Io)?;
            if n == 0 {
                return Err(ObsctlError::IpcProtocolError(
                    "connection closed before subscribe ack".to_string(),
                ));
            }
            match decode::<ServerMessage>(line.trim()) {
                Ok(ServerMessage::Response {
                    id: resp_id,
                    ok,
                    error,
                    ..
                }) if resp_id == id => {
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
                _ => {}
            }
        }
    }

    /// Read the next pushed event from the server, skipping responses.
    pub async fn next_event(&mut self) -> Result<ServerMessage> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(ObsctlError::Io)?;
            if n == 0 {
                return Err(ObsctlError::IpcProtocolError(
                    "connection closed".to_string(),
                ));
            }
            if let Ok(msg @ ServerMessage::Event { .. }) = decode::<ServerMessage>(line.trim()) {
                return Ok(msg);
            }
        }
    }
}
