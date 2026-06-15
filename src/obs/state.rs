use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsSnapshot {
    pub connected: bool,
    pub obs_studio_version: Option<String>,
    pub obs_websocket_version: Option<String>,
    pub current_scene: Option<String>,
    pub scenes: Vec<SceneState>,
    pub audio_inputs: Vec<AudioState>,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl Default for ObsSnapshot {
    fn default() -> Self {
        Self {
            connected: false,
            obs_studio_version: None,
            obs_websocket_version: None,
            current_scene: None,
            scenes: Vec::new(),
            audio_inputs: Vec::new(),
            last_error: None,
            updated_at: OffsetDateTime::now_utc(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SceneState {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
    pub group: Option<String>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioState {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
    pub kind: Option<String>,
    pub muted: Option<bool>,
    pub volume_mul: Option<f64>,
    pub volume_db: Option<f64>,
    pub volume_percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub pid: u32,
    pub uptime_seconds: u64,
    pub socket_path: std::path::PathBuf,
    pub client_count: usize,
    pub obs_connected: bool,
    pub reconnecting: bool,
    pub last_error: Option<String>,
}
