use std::collections::HashMap;

use time::OffsetDateTime;

use crate::{
    ipc::protocol::{LogEvent, LogLevel},
    obs::state::{AudioState, ObsSnapshot, SceneState, ServerStatus},
    tui::{anim::AnimClock, theme::Theme},
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FocusPanel {
    #[default]
    Scenes,
    Audio,
    Profiles,
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    pub snapshot: Option<ObsSnapshot>,
    pub server_status: Option<ServerStatus>,
    pub logs: Vec<TuiLogEntry>,
    pub command_palette: CommandPaletteState,
    pub last_result: Option<String>,
    pub connected_to_daemon: bool,
    pub focus: FocusPanel,
    pub scene_cursor: usize,
    pub audio_cursor: usize,
    pub profile_cursor: usize,
    /// Latest RMS magnitude (0-1) per input name, updated from InputVolumeMeters events.
    pub meter_levels: HashMap<String, f32>,
    /// Active color theme, chosen via config or the settings view.
    pub theme: Theme,
    /// Advances once per render tick; drives pulsing/spinner animations.
    pub anim: AnimClock,
    /// (scene name, tick it became active) — drives the brief flash
    /// highlight in the scenes panel right after a switch. Set by the
    /// event applier when it observes `current_scene` change.
    pub scene_flash: Option<(String, u64)>,
    /// Cached visible (non-hidden) scenes; rebuilt in `clamp_cursors` after each snapshot update.
    cached_visible_scenes: Vec<SceneState>,
}

impl Default for TuiModel {
    fn default() -> Self {
        Self {
            snapshot: None,
            server_status: None,
            logs: Vec::new(),
            command_palette: CommandPaletteState::default(),
            last_result: None,
            connected_to_daemon: false,
            focus: FocusPanel::default(),
            scene_cursor: 0,
            audio_cursor: 0,
            profile_cursor: 0,
            meter_levels: HashMap::new(),
            theme: Theme::default_theme(),
            anim: AnimClock::default(),
            scene_flash: None,
            cached_visible_scenes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiLogEntry {
    pub level: LogLevel,
    pub message: String,
    pub target: Option<String>,
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Default)]
pub struct CommandPaletteState {
    pub active: bool,
    pub input: String,
    pub completions: Vec<String>,
    pub completion_idx: Option<usize>,
}

impl CommandPaletteState {
    pub fn cycle_next(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let next = match self.completion_idx {
            None => 0,
            Some(i) => (i + 1) % self.completions.len(),
        };
        self.completion_idx = Some(next);
        self.input = self.completions[next].clone();
    }

    pub fn cycle_prev(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let prev = match self.completion_idx {
            None => self.completions.len() - 1,
            Some(0) => self.completions.len() - 1,
            Some(i) => i - 1,
        };
        self.completion_idx = Some(prev);
        self.input = self.completions[prev].clone();
    }
}

impl From<LogEvent> for TuiLogEntry {
    fn from(event: LogEvent) -> Self {
        Self {
            level: event.level,
            message: event.message,
            target: event.target,
            timestamp: event.timestamp,
        }
    }
}

impl TuiModel {
    pub const MAX_LOG_ENTRIES: usize = 200;

    /// Build a model with the given starting theme (used at startup, once
    /// the configured/custom theme has been resolved).
    pub fn with_theme(theme: Theme) -> Self {
        Self {
            theme,
            ..Self::default()
        }
    }

    pub fn push_log(&mut self, entry: TuiLogEntry) {
        self.logs.push(entry);
        if self.logs.len() > Self::MAX_LOG_ENTRIES {
            let overflow = self.logs.len() - Self::MAX_LOG_ENTRIES;
            self.logs.drain(0..overflow);
        }
    }

    /// Visible (non-hidden) scenes, in snapshot order. Returns the cached slice; no allocation per call.
    pub fn scenes(&self) -> &[SceneState] {
        &self.cached_visible_scenes
    }

    pub fn audio_inputs(&self) -> &[AudioState] {
        self.snapshot
            .as_ref()
            .map(|s| s.audio_inputs.as_slice())
            .unwrap_or(&[])
    }

    pub fn profiles(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .map(|s| s.profiles.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_profile(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.current_profile.as_deref())
    }

    pub fn stats(&self) -> Option<&crate::obs::state::ObsStats> {
        self.snapshot.as_ref().and_then(|s| s.stats.as_ref())
    }

    pub fn stream_bitrate_kbps(&self) -> Option<f64> {
        self.snapshot.as_ref().and_then(|s| s.stream_bitrate_kbps)
    }

    pub fn stream_duration_ms(&self) -> Option<u64> {
        self.snapshot.as_ref().and_then(|s| s.stream_duration_ms)
    }

    pub fn record_duration_ms(&self) -> Option<u64> {
        self.snapshot.as_ref().and_then(|s| s.record_duration_ms)
    }

    pub fn current_scene(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.current_scene.as_deref())
    }

    pub fn obs_connected(&self) -> bool {
        self.snapshot.as_ref().map(|s| s.connected).unwrap_or(false)
    }

    pub fn move_up(&mut self) {
        match self.focus {
            FocusPanel::Scenes => self.scene_cursor = self.scene_cursor.saturating_sub(1),
            FocusPanel::Audio => self.audio_cursor = self.audio_cursor.saturating_sub(1),
            FocusPanel::Profiles => self.profile_cursor = self.profile_cursor.saturating_sub(1),
        }
    }

    pub fn move_down(&mut self) {
        match self.focus {
            FocusPanel::Scenes => {
                let max = self.scenes().len().saturating_sub(1);
                if self.scene_cursor < max {
                    self.scene_cursor += 1;
                }
            }
            FocusPanel::Audio => {
                let max = self.audio_inputs().len().saturating_sub(1);
                if self.audio_cursor < max {
                    self.audio_cursor += 1;
                }
            }
            FocusPanel::Profiles => {
                let max = self.profiles().len().saturating_sub(1);
                if self.profile_cursor < max {
                    self.profile_cursor += 1;
                }
            }
        }
    }

    /// Keep cursors within valid list bounds; call after snapshot updates.
    pub fn clamp_cursors(&mut self) {
        self.cached_visible_scenes = self
            .snapshot
            .as_ref()
            .map(|s| s.scenes.iter().filter(|sc| !sc.hidden).cloned().collect())
            .unwrap_or_default();
        let scene_max = self.cached_visible_scenes.len().saturating_sub(1);
        self.scene_cursor = self.scene_cursor.min(scene_max);
        let audio_max = self.audio_inputs().len().saturating_sub(1);
        self.audio_cursor = self.audio_cursor.min(audio_max);
        let profile_max = self.profiles().len().saturating_sub(1);
        self.profile_cursor = self.profile_cursor.min(profile_max);
    }

    pub fn focused_scene(&self) -> Option<&SceneState> {
        self.cached_visible_scenes.get(self.scene_cursor)
    }

    pub fn focused_audio(&self) -> Option<&AudioState> {
        self.audio_inputs().get(self.audio_cursor)
    }

    pub fn focused_profile(&self) -> Option<&str> {
        self.profiles().get(self.profile_cursor).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::state::{ObsSnapshot, SceneState};

    fn make_scene(name: &str, hidden: bool) -> SceneState {
        SceneState {
            name: name.to_string(),
            hidden,
            ..Default::default()
        }
    }

    fn model_with_scenes(scenes: Vec<SceneState>) -> TuiModel {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                scenes,
                ..Default::default()
            }),
            ..Default::default()
        };
        model.clamp_cursors();
        model
    }

    #[test]
    fn scenes_excludes_hidden() {
        let model = model_with_scenes(vec![
            make_scene("visible_a", false),
            make_scene("hidden_x", true),
            make_scene("visible_b", false),
            make_scene("hidden_y", true),
        ]);

        let visible: Vec<&str> = model.scenes().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(visible, vec!["visible_a", "visible_b"]);
    }

    #[test]
    fn cursor_stays_zero_when_all_scenes_hidden() {
        let mut model = model_with_scenes(vec![
            make_scene("hidden_a", true),
            make_scene("hidden_b", true),
        ]);
        model.scene_cursor = 5; // set an out-of-bounds cursor before clamping
        model.clamp_cursors();

        assert_eq!(model.scenes().len(), 0);
        assert_eq!(model.scene_cursor, 0);
    }

    #[test]
    fn profile_navigation_clamps_and_focuses() {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                profiles: vec!["Default".to_string(), "Streaming".to_string()],
                ..Default::default()
            }),
            focus: FocusPanel::Profiles,
            ..Default::default()
        };
        model.clamp_cursors();

        assert_eq!(model.focused_profile(), Some("Default"));
        model.move_down();
        assert_eq!(model.focused_profile(), Some("Streaming"));
        model.move_down(); // already at last entry; stays put
        assert_eq!(model.focused_profile(), Some("Streaming"));
        model.move_up();
        assert_eq!(model.focused_profile(), Some("Default"));
    }
}
