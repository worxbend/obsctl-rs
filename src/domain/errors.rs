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

    #[error("ambiguous target: {0}")]
    AliasAmbiguous(String),

    #[error("command parse error: {0}")]
    CommandParseError(String),

    #[error("dump config failed: {0}")]
    DumpConfigFailed(String),

    #[error("service install failed: {0}")]
    ServiceInstallFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl ObsctlError {
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
            | Self::AudioInputNotFound(_) => 4,
            Self::CommandParseError(_) => 5,
            Self::IpcProtocolError(_) => 6,
            Self::AliasAmbiguous(_)
            | Self::DumpConfigFailed(_)
            | Self::ServiceInstallFailed(_)
            | Self::Io(_) => 1,
        }
    }
}
