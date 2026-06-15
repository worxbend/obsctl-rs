use crate::obs::state::{AudioState, ObsSnapshot, SceneState, ServerStatus};

#[derive(Debug, Clone, Default)]
pub struct TuiModel {
    pub snapshot: Option<ObsSnapshot>,
    pub server_status: Option<ServerStatus>,
    pub logs: Vec<String>,
    pub command_palette: CommandPaletteState,
    pub last_result: Option<String>,
    pub connected_to_daemon: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CommandPaletteState {
    pub active: bool,
    pub input: String,
}

impl TuiModel {
    pub fn scenes(&self) -> &[SceneState] {
        self.snapshot
            .as_ref()
            .map(|s| s.scenes.as_slice())
            .unwrap_or(&[])
    }

    pub fn audio_inputs(&self) -> &[AudioState] {
        self.snapshot
            .as_ref()
            .map(|s| s.audio_inputs.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_scene(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.current_scene.as_deref())
    }

    pub fn obs_connected(&self) -> bool {
        self.snapshot.as_ref().map(|s| s.connected).unwrap_or(false)
    }
}
