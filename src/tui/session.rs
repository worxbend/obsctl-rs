use std::path::Path;

use crate::{
    domain::result::Result,
    ipc::{
        protocol::{CommandPayload, ServerMessage, TOPIC_EVENTS, TOPIC_LOGS, TOPIC_STATE},
        unix_client::IpcClient,
    },
};

/// Long-lived subscription session — receives pushed events only.
pub struct TuiEventSession {
    client: IpcClient,
}

impl TuiEventSession {
    pub async fn connect(path: &Path) -> Result<Self> {
        let mut client = IpcClient::connect(path).await?;
        client
            .subscribe(&[TOPIC_STATE, TOPIC_EVENTS, TOPIC_LOGS])
            .await?;
        Ok(Self { client })
    }

    pub async fn next_event(&mut self) -> Result<ServerMessage> {
        self.client.next_event().await
    }
}

/// Short-lived connection for sending a single command and reading a response.
pub async fn send_command(path: &Path, payload: CommandPayload) -> Result<ServerMessage> {
    let mut client = IpcClient::connect(path).await?;
    client.send_command(payload).await
}
