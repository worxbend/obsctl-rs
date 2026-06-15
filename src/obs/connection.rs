use tokio::sync::mpsc;

use crate::config::model::ConnectionConfig;
use crate::domain::{errors::ObsctlError, result::Result};
use crate::obs::client::{handshake, ObsClient, ObsEvent};

/// Derived connection parameters, with password resolved from env if needed.
pub struct ObsConnectionParams {
    pub url: String,
    pub password: Option<String>,
    pub connect_timeout_ms: u64,
}

impl ObsConnectionParams {
    pub fn from_config(config: &ConnectionConfig) -> Self {
        let password = if let Some(ref pw) = config.password {
            Some(pw.clone())
        } else if !config.password_env.is_empty() {
            std::env::var(&config.password_env).ok()
        } else {
            None
        };
        Self {
            url: format!("ws://{}:{}", config.host, config.port),
            password,
            connect_timeout_ms: config.connect_timeout_ms,
        }
    }
}

/// Connect to OBS WebSocket, complete the handshake, and return a client handle.
/// Returns `(client, obs_studio_version, obs_websocket_version)`.
pub async fn connect(
    params: &ObsConnectionParams,
    event_tx: mpsc::Sender<ObsEvent>,
) -> Result<(ObsClient, String, String)> {
    let (ws_stream, _) = tokio::time::timeout(
        std::time::Duration::from_millis(params.connect_timeout_ms),
        tokio_tungstenite::connect_async(&params.url),
    )
    .await
    .map_err(|_| ObsctlError::ConnectionFailed(format!("connect timeout to {}", params.url)))?
    .map_err(|e| ObsctlError::ConnectionFailed(e.to_string()))?;

    let (sink, stream) = futures_util::StreamExt::split(ws_stream);
    handshake(sink, stream, params.password.as_deref(), event_tx).await
}
