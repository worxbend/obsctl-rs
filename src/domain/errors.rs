use thiserror::Error;

#[derive(Debug, Error)]
pub enum ObsctlError {
    #[error("config not found at {0}")]
    ConfigNotFound(String),

    #[error("config invalid: {0}")]
    ConfigInvalid(String),

    #[error("server unavailable at {socket_path}: {message}")]
    ServerUnavailable {
        socket_path: String,
        message: String,
    },

    #[error("IPC connection failed: {0}")]
    IpcConnectionFailed(String),

    #[error("IPC protocol error: {0}")]
    IpcProtocolError(String),

    /// The daemon accepted the connection but did not answer in time.
    ///
    /// Distinct from `IpcConnectionFailed` (no daemon listening) and from
    /// `RequestTimeout` (OBS did not answer the daemon): here the daemon is
    /// running and reachable but wedged, so the only thing the client can do
    /// is give up and say so rather than wait forever.
    #[error("daemon did not respond within {seconds}s")]
    IpcTimeout { seconds: u64 },

    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("authentication failed")]
    AuthenticationFailed,

    #[error("OBS unavailable")]
    ObsUnavailable,

    #[error("request timed out")]
    RequestTimeout,

    #[error("OBS request failed: {0}")]
    ObsRequestFailed(String),

    #[error("scene not found: {0}")]
    SceneNotFound(String),

    #[error("audio input not found: {0}")]
    AudioInputNotFound(String),

    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    #[error("scene collection not found: {0}")]
    SceneCollectionNotFound(String),

    #[error("ambiguous target: {0}")]
    AliasAmbiguous(String),

    #[error("command parse error: {0}")]
    CommandParseError(String),

    #[error("remote shutdown is not enabled in config")]
    ShutdownDisabled,

    #[error("dump config failed: {0}")]
    DumpConfigFailed(String),

    #[error("service install failed: {0}")]
    ServiceInstallFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl ObsctlError {
    /// Exit code for failures handled by the current local process.
    ///
    /// This mapping is intentionally separate from the public IPC proxy
    /// mapping in `crate::ipc::protocol::PublicErrorCode`. Local commands can
    /// fail before a daemon response exists, while proxy commands map
    /// daemon-supplied public error codes through the stable IPC contract.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::ConfigNotFound(_) | Self::ConfigInvalid(_) => 2,
            Self::ServerUnavailable { .. }
            | Self::IpcConnectionFailed(_)
            | Self::ConnectionFailed(_)
            | Self::AuthenticationFailed => 3,
            Self::ObsUnavailable
            | Self::ObsRequestFailed(_)
            | Self::RequestTimeout
            | Self::SceneNotFound(_)
            | Self::AudioInputNotFound(_)
            | Self::ProfileNotFound(_)
            | Self::SceneCollectionNotFound(_) => 4,
            Self::CommandParseError(_) => 5,
            Self::IpcProtocolError(_) | Self::IpcTimeout { .. } => 6,
            Self::AliasAmbiguous(_)
            | Self::ShutdownDisabled
            | Self::DumpConfigFailed(_)
            | Self::ServiceInstallFailed(_)
            | Self::Io(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time exhaustiveness guard: fails to compile when a new variant is
    // added without updating this match, forcing the test cases to be updated too.
    #[allow(dead_code)]
    fn _obsctl_error_variant_guard(e: ObsctlError) {
        match e {
            ObsctlError::ConfigNotFound(_) => {}
            ObsctlError::ConfigInvalid(_) => {}
            ObsctlError::ServerUnavailable { .. } => {}
            ObsctlError::IpcConnectionFailed(_) => {}
            ObsctlError::IpcProtocolError(_) => {}
            ObsctlError::IpcTimeout { .. } => {}
            ObsctlError::ConnectionFailed(_) => {}
            ObsctlError::AuthenticationFailed => {}
            ObsctlError::ObsUnavailable => {}
            ObsctlError::RequestTimeout => {}
            ObsctlError::ObsRequestFailed(_) => {}
            ObsctlError::SceneNotFound(_) => {}
            ObsctlError::AudioInputNotFound(_) => {}
            ObsctlError::ProfileNotFound(_) => {}
            ObsctlError::SceneCollectionNotFound(_) => {}
            ObsctlError::AliasAmbiguous(_) => {}
            ObsctlError::CommandParseError(_) => {}
            ObsctlError::ShutdownDisabled => {}
            ObsctlError::DumpConfigFailed(_) => {}
            ObsctlError::ServiceInstallFailed(_) => {}
            ObsctlError::Io(_) => {}
        }
    }

    #[test]
    fn all_obsctl_error_variants_have_intended_local_exit_codes() {
        const OBSCTL_ERROR_VARIANT_COUNT: usize = 21;

        let cases = [
            (
                ObsctlError::ConfigNotFound("/path".to_string()),
                2,
                "config not found",
            ),
            (
                ObsctlError::ConfigInvalid("bad".to_string()),
                2,
                "config invalid",
            ),
            (
                ObsctlError::ServerUnavailable {
                    socket_path: "/s".to_string(),
                    message: "refused".to_string(),
                },
                3,
                "server unavailable",
            ),
            (
                ObsctlError::IpcConnectionFailed("x".to_string()),
                3,
                "IPC connection failed",
            ),
            (
                ObsctlError::IpcProtocolError("bad frame".to_string()),
                6,
                "IPC protocol error",
            ),
            (
                ObsctlError::IpcTimeout { seconds: 30 },
                6,
                "daemon did not respond",
            ),
            (
                ObsctlError::ConnectionFailed("closed".to_string()),
                3,
                "OBS connection failed",
            ),
            (
                ObsctlError::AuthenticationFailed,
                3,
                "OBS authentication failed",
            ),
            (ObsctlError::ObsUnavailable, 4, "OBS unavailable"),
            (ObsctlError::RequestTimeout, 4, "request timeout"),
            (
                ObsctlError::ObsRequestFailed("request failed".to_string()),
                4,
                "OBS request failed",
            ),
            (
                ObsctlError::SceneNotFound("main".to_string()),
                4,
                "scene not found",
            ),
            (
                ObsctlError::AudioInputNotFound("mic".to_string()),
                4,
                "audio input not found",
            ),
            (
                ObsctlError::ProfileNotFound("Streaming".to_string()),
                4,
                "profile not found",
            ),
            (
                ObsctlError::SceneCollectionNotFound("Podcast".to_string()),
                4,
                "scene collection not found",
            ),
            (
                ObsctlError::AliasAmbiguous("cam".to_string()),
                1,
                "ambiguous alias",
            ),
            (
                ObsctlError::CommandParseError("bad".to_string()),
                5,
                "command parse error",
            ),
            (ObsctlError::ShutdownDisabled, 1, "shutdown disabled"),
            (
                ObsctlError::DumpConfigFailed("write failed".to_string()),
                1,
                "dump config failed",
            ),
            (
                ObsctlError::ServiceInstallFailed("systemctl failed".to_string()),
                1,
                "service install failed",
            ),
            (
                ObsctlError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "disk failed",
                )),
                1,
                "IO error",
            ),
        ];

        assert_eq!(cases.len(), OBSCTL_ERROR_VARIANT_COUNT);

        for (error, expected_exit_code, description) in cases {
            assert_eq!(error.exit_code(), expected_exit_code, "{description}");
        }
    }

    #[test]
    fn config_errors_map_to_exit_2() {
        assert_eq!(
            ObsctlError::ConfigNotFound("/path".to_string()).exit_code(),
            2
        );
        assert_eq!(ObsctlError::ConfigInvalid("bad".to_string()).exit_code(), 2);
    }

    #[test]
    fn server_errors_map_to_exit_3() {
        assert_eq!(
            ObsctlError::ServerUnavailable {
                socket_path: "/s".to_string(),
                message: "refused".to_string()
            }
            .exit_code(),
            3
        );
        assert_eq!(
            ObsctlError::IpcConnectionFailed("x".to_string()).exit_code(),
            3
        );
        assert_eq!(ObsctlError::AuthenticationFailed.exit_code(), 3);
    }

    #[test]
    fn obs_errors_map_to_exit_4() {
        assert_eq!(ObsctlError::ObsUnavailable.exit_code(), 4);
        assert_eq!(ObsctlError::RequestTimeout.exit_code(), 4);
        assert_eq!(ObsctlError::SceneNotFound("x".to_string()).exit_code(), 4);
        assert_eq!(
            ObsctlError::AudioInputNotFound("x".to_string()).exit_code(),
            4
        );
        assert_eq!(ObsctlError::ProfileNotFound("x".to_string()).exit_code(), 4);
        assert_eq!(
            ObsctlError::SceneCollectionNotFound("x".to_string()).exit_code(),
            4
        );
    }

    #[test]
    fn parse_error_maps_to_exit_5() {
        assert_eq!(
            ObsctlError::CommandParseError("bad".to_string()).exit_code(),
            5
        );
    }

    #[test]
    fn ipc_protocol_error_maps_to_exit_6() {
        assert_eq!(
            ObsctlError::IpcProtocolError("bad frame".to_string()).exit_code(),
            6
        );
    }

    #[test]
    fn shutdown_disabled_maps_to_generic_exit_1() {
        assert_eq!(ObsctlError::ShutdownDisabled.exit_code(), 1);
    }

    #[test]
    fn error_messages_do_not_leak_secrets() {
        let err = ObsctlError::AuthenticationFailed;
        let msg = err.to_string();
        assert!(!msg.contains("password"), "error must not mention password");
        assert!(!msg.contains("secret"), "error must not mention secret");
    }
}
