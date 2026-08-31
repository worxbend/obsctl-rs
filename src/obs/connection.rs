use tokio::sync::mpsc;

use crate::config::model::ConnectionConfig;
use crate::config::schema::validate_connection_config;
use crate::domain::{errors::ObsctlError, result::Result};
use crate::obs::client::{ObsEvent, ObsSession, handshake};
use crate::support::validation::resolve_connection_password;

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
    pub fn from_config(config: &ConnectionConfig) -> Result<Self> {
        validate_connection_config(config)?;

        let password =
            resolve_connection_password(config.password.as_deref(), &config.password_env).map_err(
                |error| ObsctlError::ConfigInvalid(error.config_field_message().to_string()),
            )?;

        Ok(Self {
            url: format!("ws://{}:{}", config.host, config.port),
            password,
            connect_timeout_ms: config.connect_timeout_ms,
            request_timeout_ms: config.request_timeout_ms,
        })
    }
}

/// Connect to OBS WebSocket, complete the handshake, and return a client handle.
pub async fn connect(
    params: &ObsConnectionParams,
    event_tx: mpsc::Sender<ObsEvent>,
) -> Result<ObsSession> {
    let (ws_stream, _) = tokio::time::timeout(
        std::time::Duration::from_millis(params.connect_timeout_ms),
        tokio_tungstenite::connect_async(&params.url),
    )
    .await
    .map_err(|_| ObsctlError::ConnectionFailed(format!("connect timeout to {}", params.url)))?
    .map_err(|e| ObsctlError::ConnectionFailed(e.to_string()))?;

    let (sink, stream) = futures_util::StreamExt::split(ws_stream);

    // The handshake gets the same budget as the socket did, because a client
    // is not connected to OBS until it has been identified. Without this, an
    // OBS that accepts the TCP connection and then never sends Hello — a hung
    // process, a network that black-holes traffic after connect, a proxy in
    // the middle — leaves the read waiting forever, and the daemon that owns
    // the OBS connection can be neither reconnected nor shut down. Expiry is
    // reported as an ordinary connection failure so the supervisor's usual
    // reconnect and backoff handle it like any other failed attempt. The two
    // phases are budgeted separately, so a whole attempt can take up to twice
    // `connect_timeout_ms` — a slow-but-healthy OBS should not lose its
    // handshake budget to a slow socket.
    tokio::time::timeout(
        std::time::Duration::from_millis(params.connect_timeout_ms),
        handshake(
            sink,
            stream,
            params.password.as_deref(),
            event_tx,
            params.request_timeout_ms,
        ),
    )
    .await
    .map_err(|_| ObsctlError::ConnectionFailed(format!("handshake timeout to {}", params.url)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::validation::MAX_PASSWORD_LENGTH;
    use crate::support::validation::test_env::{with_env_var, with_env_var_os};

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
        let p = ObsConnectionParams::from_config(&cfg).unwrap();
        assert_eq!(p.password.as_deref(), Some("direct"));
        let debug = format!("{p:?}");
        assert!(!debug.contains("direct"));
    }

    #[test]
    fn from_config_resolves_env_password() {
        with_env_var("TEST_CONN_PARAMS_PW", Some("envpw"), || {
            let cfg = ConnectionConfig {
                password: None,
                password_env: "TEST_CONN_PARAMS_PW".to_string(),
                ..ConnectionConfig::default()
            };
            let p = ObsConnectionParams::from_config(&cfg).unwrap();
            assert_eq!(p.password.as_deref(), Some("envpw"));
        });
    }

    #[test]
    fn from_config_rejects_plaintext_password_and_password_env() {
        let cfg = ConnectionConfig {
            password: Some("direct".to_string()),
            password_env: "OBS_WEBSOCKET_PASSWORD".to_string(),
            ..ConnectionConfig::default()
        };
        assert!(ObsConnectionParams::from_config(&cfg).is_err());
    }

    #[test]
    fn from_config_rejects_control_characters_in_plaintext_password() {
        let cfg = ConnectionConfig {
            password: Some("abc\t123".to_string()),
            password_env: String::new(),
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(err.to_string().contains("control characters"));
    }

    #[cfg(unix)]
    #[test]
    fn from_config_rejects_non_unicode_password_env() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let env_name = "TEST_CONN_PARAMS_NON_UNI";
        let value = OsString::from_vec(vec![0xff]);
        with_env_var_os(env_name, Some(value), || {
            let cfg = ConnectionConfig {
                password: None,
                password_env: env_name.to_string(),
                ..ConnectionConfig::default()
            };
            let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
            assert!(err.to_string().contains("valid UTF-8"));
        });
    }

    #[test]
    fn from_config_rejects_control_characters_in_password_env_value() {
        with_env_var("TEST_CONN_PARAMS_CTRL", Some("abc\t123"), || {
            let cfg = ConnectionConfig {
                password: None,
                password_env: "TEST_CONN_PARAMS_CTRL".to_string(),
                ..ConnectionConfig::default()
            };
            let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
            assert!(err.to_string().contains("control characters"));
        });
    }

    #[test]
    fn from_config_rejects_oversized_plaintext_password() {
        let cfg = ConnectionConfig {
            password: Some("a".repeat(MAX_PASSWORD_LENGTH + 1)),
            password_env: String::new(),
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("connection.password exceeds maximum length")
        );
    }

    #[test]
    fn from_config_rejects_oversized_password_env_value() {
        with_env_var(
            "TEST_CONN_PARAMS_LONG_ENV_PW",
            Some(&"a".repeat(MAX_PASSWORD_LENGTH + 1)),
            || {
                let cfg = ConnectionConfig {
                    password: None,
                    password_env: "TEST_CONN_PARAMS_LONG_ENV_PW".to_string(),
                    ..ConnectionConfig::default()
                };
                let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
                assert!(
                    err.to_string()
                        .contains("connection.password_env value exceeds maximum length")
                );
            },
        );
    }

    #[test]
    fn from_config_rejects_invalid_password_env() {
        let cfg = ConnectionConfig {
            password: None,
            password_env: "1BAD".to_string(),
            ..ConnectionConfig::default()
        };
        assert!(ObsConnectionParams::from_config(&cfg).is_err());
    }

    #[test]
    fn from_config_rejects_invalid_host_characters() {
        let cfg = ConnectionConfig {
            host: "127.0.0.1\n".to_string(),
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(err.to_string().contains("control or whitespace"));
    }

    #[test]
    fn from_config_rejects_host_with_reserved_port_delimiter() {
        let cfg = ConnectionConfig {
            host: "127.0.0.1:4455".to_string(),
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("path, separator, userinfo, or colon")
        );
    }

    #[test]
    fn from_config_rejects_zero_port() {
        let cfg = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("connection.port must be in range 1-65535")
        );
    }

    #[test]
    fn from_config_rejects_zero_connect_timeout() {
        let cfg = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            connect_timeout_ms: 0,
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("connection.connect_timeout_ms must be positive")
        );
    }

    #[test]
    fn from_config_rejects_zero_request_timeout() {
        let cfg = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            request_timeout_ms: 0,
            ..ConnectionConfig::default()
        };
        let err = ObsConnectionParams::from_config(&cfg).unwrap_err();
        assert!(
            err.to_string()
                .contains("connection.request_timeout_ms must be positive")
        );
    }
}
