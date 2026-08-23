#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Quit,
    Themes,
    DumpConfig,
    ReloadConfig,
    Status,
    ServerStatus,
    ObsStatus,
    ValidateConfig,
    Reconnect,
    Connect,
    ShutdownServer,
    SetScene {
        target: String,
    },
    SetProfile {
        target: String,
    },
    SetSceneCollection {
        target: String,
    },
    /// Switch to a named obsctl *scene* profile — a set of scene-visibility
    /// choices. Not [`Command::SetProfile`], which switches the OBS profile.
    SetSceneProfile {
        target: String,
    },
    /// Stop using any scene profile, so the per-scene `hidden` flags decide
    /// again.
    ClearSceneProfile,
    Mute {
        target: String,
    },
    Unmute {
        target: String,
    },
    ToggleMute {
        target: String,
    },
    SetVolume {
        target: String,
        percent: u8,
    },
    ToggleStream,
    ToggleRecord,
}
