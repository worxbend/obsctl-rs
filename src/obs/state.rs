use serde::{Deserialize, Serialize};

use crate::domain::volume::{mul_to_db, mul_to_percent};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsSnapshot {
    pub connected: bool,
    pub obs_studio_version: Option<String>,
    pub obs_websocket_version: Option<String>,
    pub current_scene: Option<String>,
    pub scenes: Vec<SceneState>,
    pub audio_inputs: Vec<AudioState>,
    pub streaming: bool,
    pub recording: bool,
    /// All OBS profile names (`GetProfileList.profiles`).
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Name of the currently active OBS profile.
    #[serde(default)]
    pub current_profile: Option<String>,
    /// All OBS scene collection names (`GetSceneCollectionList.sceneCollections`).
    #[serde(default)]
    pub scene_collections: Vec<String>,
    /// Name of the currently active OBS scene collection.
    #[serde(default)]
    pub current_scene_collection: Option<String>,
    pub last_error: Option<String>,
    /// CPU/memory/disk/render performance stats, polled periodically from
    /// `GetStats`. `None` until the first poll completes after connecting.
    #[serde(default)]
    pub stats: Option<ObsStats>,
    /// Stream encoder bitrate in kbit/s, derived from the `GetStreamStatus`
    /// `outputBytes` delta between polls (obs-websocket has no direct
    /// bitrate field). `None` while not streaming or before the second poll.
    #[serde(default)]
    pub stream_bitrate_kbps: Option<f64>,
    /// Elapsed stream time in milliseconds (`GetStreamStatus.outputDuration`).
    #[serde(default)]
    pub stream_duration_ms: Option<u64>,
    /// Elapsed recording time in milliseconds (`GetRecordStatus.outputDuration`).
    #[serde(default)]
    pub record_duration_ms: Option<u64>,
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
            streaming: false,
            recording: false,
            profiles: Vec::new(),
            current_profile: None,
            scene_collections: Vec::new(),
            current_scene_collection: None,
            last_error: None,
            stats: None,
            stream_bitrate_kbps: None,
            stream_duration_ms: None,
            record_duration_ms: None,
            updated_at: OffsetDateTime::now_utc(),
        }
    }
}

/// Point-in-time performance stats from OBS's `GetStats` request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct ObsStats {
    pub cpu_usage_percent: f64,
    pub memory_usage_mb: f64,
    pub available_disk_space_mb: f64,
    pub active_fps: f64,
    pub average_frame_render_time_ms: f64,
    pub render_skipped_frames: u64,
    pub render_total_frames: u64,
    pub output_skipped_frames: u64,
    pub output_total_frames: u64,
}

impl ObsStats {
    /// Parse a `GetStats` `responseData` JSON value; missing/malformed
    /// fields default to `0.0`/`0` rather than failing the whole snapshot.
    pub fn from_response(v: &serde_json::Value) -> Self {
        let f = |key: &str| v.get(key).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let u = |key: &str| v.get(key).and_then(|x| x.as_u64()).unwrap_or(0);
        Self {
            cpu_usage_percent: f("cpuUsage"),
            memory_usage_mb: f("memoryUsage"),
            available_disk_space_mb: f("availableDiskSpace"),
            active_fps: f("activeFps"),
            average_frame_render_time_ms: f("averageFrameRenderTime"),
            render_skipped_frames: u("renderSkippedFrames"),
            render_total_frames: u("renderTotalFrames"),
            output_skipped_frames: u("outputSkippedFrames"),
            output_total_frames: u("outputTotalFrames"),
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
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioState {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
    pub kind: Option<String>,
    pub muted: Option<bool>,
    /// One level in three representations — see [`AudioState::set_level`].
    /// Public because serde carries them across the IPC boundary; set them
    /// through the methods rather than individually.
    pub volume_mul: Option<f64>,
    pub volume_db: Option<f64>,
    pub volume_percent: Option<u8>,
}

impl AudioState {
    /// Set this input's level from a linear multiplier, deriving the decibel
    /// and percentage views of it.
    ///
    /// The three fields are one quantity written three ways: the multiplier
    /// OBS works in, the decibels the audio panel prints, and the percentage
    /// the volume commands use. They were maintained by three separate blocks
    /// of three assignments each, and `widgets::audio` reads two of them
    /// independently — so a writer that set the multiplier and forgot the
    /// decibels would render "60%" beside a stale "-12.0 dB" with nothing
    /// failing.
    pub fn set_level(&mut self, volume_mul: f64) {
        self.set_level_with_db(volume_mul, mul_to_db(volume_mul));
    }

    /// As [`AudioState::set_level`], but keeping the decibel value OBS
    /// reported rather than deriving it.
    ///
    /// OBS sends both in `InputVolumeChanged`, and its dB is not exactly
    /// `mul_to_db` of its multiplier — it rounds. Recomputing would change the
    /// number the daemon publishes for a level OBS already described, which an
    /// integration test pins (`inputVolumeMul: 0.42` is reported as `-7.5`,
    /// while `mul_to_db(0.42)` is -7.535…).
    pub fn set_level_with_db(&mut self, volume_mul: f64, volume_db: f64) {
        self.volume_mul = Some(volume_mul);
        self.volume_db = Some(volume_db);
        self.volume_percent = Some(mul_to_percent(volume_mul));
    }

    /// Forget the level, for an input OBS has not reported one for.
    pub fn clear_level(&mut self) {
        self.volume_mul = None;
        self.volume_db = None;
        self.volume_percent = None;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obs_stats_from_response_parses_all_fields() {
        let v = serde_json::json!({
            "cpuUsage": 12.5,
            "memoryUsage": 512.0,
            "availableDiskSpace": 100_000.0,
            "activeFps": 59.94,
            "averageFrameRenderTime": 3.2,
            "renderSkippedFrames": 4,
            "renderTotalFrames": 10_000,
            "outputSkippedFrames": 1,
            "outputTotalFrames": 9_999,
        });
        let stats = ObsStats::from_response(&v);
        assert_eq!(stats.cpu_usage_percent, 12.5);
        assert_eq!(stats.memory_usage_mb, 512.0);
        assert_eq!(stats.active_fps, 59.94);
        assert_eq!(stats.render_skipped_frames, 4);
        assert_eq!(stats.output_total_frames, 9_999);
    }

    #[test]
    fn obs_stats_from_response_defaults_missing_fields() {
        let stats = ObsStats::from_response(&serde_json::json!({}));
        assert_eq!(stats, ObsStats::default());
    }
}
