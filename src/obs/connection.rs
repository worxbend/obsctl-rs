use tokio::sync::mpsc;

use crate::config::model::ConnectionConfig;
use crate::domain::{errors::ObsctlError, result::Result};
use crate::obs::client::{ObsClient, ObsEvent, handshake};

/// Derived connection parameters, with password resolved from env if needed.
pub struct ObsConnectionParams {
    pub url: String,
    pub password: Option<String>,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
}

impl std::fmt::Debug for ObsConnectionParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObsConnectionParams")
            .field("url", &self.url)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .finish()
    }
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
            request_timeout_ms: config.request_timeout_ms,
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
    handshake(
        sink,
        stream,
        params.password.as_deref(),
        event_tx,
        params.request_timeout_ms,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obs_connection_params_debug_redacts_password() {
        let params = ObsConnectionParams {
            url: "ws://127.0.0.1:4455".to_string(),
            password: Some("mysecret".to_string()),
            connect_timeout_ms: 3000,
            request_timeout_ms: 2500,
        };
        let debug = format!("{params:?}");
        assert!(
            !debug.contains("mysecret"),
            "debug must not expose password: {debug}"
        );
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn obs_connection_params_debug_shows_none_when_no_password() {
        let params = ObsConnectionParams {
            url: "ws://127.0.0.1:4455".to_string(),
            password: None,
            connect_timeout_ms: 3000,
            request_timeout_ms: 2500,
        };
        let debug = format!("{params:?}");
        assert!(debug.contains("None"));
    }

    #[test]
    fn from_config_resolves_plaintext_password() {
        let cfg = ConnectionConfig {
            password: Some("direct".to_string()),
            password_env: String::new(),
            ..ConnectionConfig::default()
        };
        let p = ObsConnectionParams::from_config(&cfg);
        assert_eq!(p.password.as_deref(), Some("direct"));
        let debug = format!("{p:?}");
        assert!(!debug.contains("direct"));
    }

    #[test]
    fn from_config_resolves_env_password() {
        unsafe { std::env::set_var("TEST_CONN_PARAMS_PW", "envpw") };
        let cfg = ConnectionConfig {
            password: None,
            password_env: "TEST_CONN_PARAMS_PW".to_string(),
            ..ConnectionConfig::default()
        };
        let p = ObsConnectionParams::from_config(&cfg);
        assert_eq!(p.password.as_deref(), Some("envpw"));
        unsafe { std::env::remove_var("TEST_CONN_PARAMS_PW") };
    }
}
