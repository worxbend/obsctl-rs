use std::sync::Arc;

use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::model::{AudioInputConfig, SceneConfig};
use crate::domain::volume::{mul_to_db, mul_to_percent};
use crate::ipc::{
    protocol::{ServerMessage, Topic},
    session::BroadcastHub,
};
use crate::obs::client::ObsEvent;
use crate::obs::state::{AudioState, ObsSnapshot, ObsStats, SceneState};

#[derive(Clone)]
pub struct StateStore {
    inner: Arc<RwLock<ObsSnapshot>>,
    hub: Arc<BroadcastHub>,
}

impl StateStore {
    pub fn new(hub: Arc<BroadcastHub>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ObsSnapshot::default())),
            hub,
        }
    }

    pub async fn read(&self) -> ObsSnapshot {
        self.inner.read().await.clone()
    }

    /// Replace the entire snapshot and broadcast.
    pub async fn replace(&self, snapshot: ObsSnapshot) {
        let mut guard = self.inner.write().await;
        *guard = snapshot;
        drop(guard);
        self.broadcast().await;
    }

    /// Mark OBS as disconnected, record error, and broadcast.
    pub async fn mark_disconnected(&self, error: Option<String>) {
        let mut guard = self.inner.write().await;
        guard.connected = false;
        guard.last_error = error;
        guard.updated_at = OffsetDateTime::now_utc();
        drop(guard);
        self.broadcast().await;
    }

    /// Apply a single OBS event to the cached snapshot and broadcast.
    pub async fn apply_event(&self, event: ObsEvent) {
        let changed = {
            let mut guard = self.inner.write().await;
            apply_to_snapshot(&mut guard, event)
        };
        if changed {
            self.broadcast().await;
        }
    }

    async fn broadcast(&self) {
        let snapshot = self.inner.read().await.clone();
        let data = match serde_json::to_value(&snapshot) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("Failed to serialize snapshot for broadcast: {e}");
                return;
            }
        };
        let msg = ServerMessage::Event {
            topic: Topic::State,
            data,
        };
        self.hub.publish(Topic::State, msg);
        debug!("State broadcast: connected={}", snapshot.connected);
    }

    /// Update the polled performance/bitrate/duration metrics and broadcast.
    /// Called periodically by the stats poller; leaves everything else
    /// (scenes, audio, streaming/recording flags) untouched.
    pub async fn update_stats(
        &self,
        stats: ObsStats,
        stream_bitrate_kbps: Option<f64>,
        stream_duration_ms: Option<u64>,
        record_duration_ms: Option<u64>,
    ) {
        let mut guard = self.inner.write().await;
        guard.stats = Some(stats);
        guard.stream_bitrate_kbps = stream_bitrate_kbps;
        guard.stream_duration_ms = stream_duration_ms;
        guard.record_duration_ms = record_duration_ms;
        guard.updated_at = OffsetDateTime::now_utc();
        drop(guard);
        self.broadcast().await;
    }

    /// Merge scene and audio config metadata into the snapshot.
    /// Called after config load/reload to attach aliases/shortcuts/groups.
    pub async fn merge_config(&self, scenes: &[SceneConfig], audio_inputs: &[AudioInputConfig]) {
        let mut guard = self.inner.write().await;
        for s in guard.scenes.iter_mut() {
            if let Some(cfg) = scenes.iter().find(|c| c.name == s.name) {
                s.alias = cfg.alias.clone();
                s.shortcut = cfg.shortcut.clone();
                s.group = cfg.group.clone();
                s.hidden = cfg.hidden;
            } else {
                s.hidden = false;
            }
        }
        for a in guard.audio_inputs.iter_mut() {
            if let Some(cfg) = audio_inputs.iter().find(|c| c.name == a.name) {
                a.alias = cfg.alias.clone();
                a.shortcut = cfg.shortcut.clone();
            }
        }
        guard.updated_at = OffsetDateTime::now_utc();
    }
}

fn apply_to_snapshot(snapshot: &mut ObsSnapshot, event: ObsEvent) -> bool {
    match event {
        ObsEvent::CurrentProgramSceneChanged { scene_name } => {
            if snapshot.current_scene.as_deref() == Some(&scene_name) {
                return false;
            }
            for s in snapshot.scenes.iter_mut() {
                s.active = s.name == scene_name;
            }
            snapshot.current_scene = Some(scene_name);
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        ObsEvent::SceneListChanged => {
            // Full refresh needed; the supervisor will fetch a new snapshot.
            true
        }
        ObsEvent::InputCreated { input_name } => {
            if snapshot.audio_inputs.iter().any(|a| a.name == input_name) {
                return false;
            }
            snapshot.audio_inputs.push(AudioState {
                name: input_name,
                ..AudioState::default()
            });
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        ObsEvent::InputRemoved { input_name } => {
            let before = snapshot.audio_inputs.len();
            snapshot.audio_inputs.retain(|a| a.name != input_name);
            if snapshot.audio_inputs.len() == before {
                return false;
            }
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        ObsEvent::InputMuteStateChanged { input_name, muted } => {
            if let Some(a) = snapshot
                .audio_inputs
                .iter_mut()
                .find(|a| a.name == input_name)
            {
                if a.muted == Some(muted) {
                    return false;
                }
                a.muted = Some(muted);
                snapshot.updated_at = OffsetDateTime::now_utc();
                true
            } else {
                false
            }
        }
        ObsEvent::InputVolumeChanged {
            input_name,
            volume_mul,
            volume_db,
        } => {
            if let Some(a) = snapshot
                .audio_inputs
                .iter_mut()
                .find(|a| a.name == input_name)
            {
                a.volume_mul = Some(volume_mul);
                a.volume_db = Some(volume_db);
                a.volume_percent = Some(mul_to_percent(volume_mul));
                snapshot.updated_at = OffsetDateTime::now_utc();
                true
            } else {
                false
            }
        }
        ObsEvent::StreamStateChanged { active } => {
            if snapshot.streaming == active {
                return false;
            }
            snapshot.streaming = active;
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        ObsEvent::RecordStateChanged { active } => {
            if snapshot.recording == active {
                return false;
            }
            snapshot.recording = active;
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        // High-frequency; don't update the snapshot or broadcast.
        ObsEvent::InputVolumeMeters { .. } => false,
        ObsEvent::Other { .. } => false,
    }
}

/// Build a full ObsSnapshot from OBS data plus config metadata.
#[allow(clippy::too_many_arguments)]
pub fn build_snapshot(
    obs_studio_version: &str,
    obs_ws_version: &str,
    scenes: &[String],
    current_scene: &str,
    inputs_raw: &[(String, Option<bool>, Option<f64>)],
    scene_cfgs: &[SceneConfig],
    audio_cfgs: &[AudioInputConfig],
    streaming: bool,
    recording: bool,
) -> ObsSnapshot {
    let scenes: Vec<SceneState> = scenes
        .iter()
        .map(|name| {
            let cfg = scene_cfgs.iter().find(|c| c.name == *name);
            SceneState {
                name: name.clone(),
                alias: cfg.and_then(|c| c.alias.clone()),
                shortcut: cfg.and_then(|c| c.shortcut.clone()),
                group: cfg.and_then(|c| c.group.clone()),
                active: *name == current_scene,
                hidden: cfg.map(|c| c.hidden).unwrap_or(false),
            }
        })
        .collect();

    let audio_inputs: Vec<AudioState> = inputs_raw
        .iter()
        .map(|(name, muted, vol_mul)| {
            let cfg = audio_cfgs.iter().find(|c| c.name == *name);
            let vol_mul_val = *vol_mul;
            AudioState {
                name: name.clone(),
                alias: cfg.and_then(|c| c.alias.clone()),
                shortcut: cfg.and_then(|c| c.shortcut.clone()),
                kind: None,
                muted: *muted,
                volume_mul: vol_mul_val,
                volume_db: vol_mul_val.map(mul_to_db),
                volume_percent: vol_mul_val.map(mul_to_percent),
            }
        })
        .collect();

    ObsSnapshot {
        connected: true,
        obs_studio_version: Some(obs_studio_version.to_string()),
        obs_websocket_version: Some(obs_ws_version.to_string()),
        current_scene: Some(current_scene.to_string()),
        scenes,
        audio_inputs,
        streaming,
        recording,
        updated_at: OffsetDateTime::now_utc(),
        ..ObsSnapshot::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::session::BroadcastHub;

    fn make_store() -> StateStore {
        let hub = Arc::new(BroadcastHub::new());
        StateStore::new(hub)
    }

    #[tokio::test]
    async fn replace_broadcasts_state() {
        let store = make_store();
        let mut rx = store.hub.subscribe_state();

        let snap = ObsSnapshot {
            connected: true,
            current_scene: Some("Main".to_string()),
            ..ObsSnapshot::default()
        };
        store.replace(snap).await;

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");

        match msg {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, Topic::State);
                assert_eq!(data["connected"], true);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn mark_disconnected_clears_connected() {
        let store = make_store();
        let snap = ObsSnapshot {
            connected: true,
            ..ObsSnapshot::default()
        };
        store.replace(snap).await;

        store.mark_disconnected(Some("timeout".to_string())).await;

        let current = store.read().await;
        assert!(!current.connected);
        assert_eq!(current.last_error.as_deref(), Some("timeout"));
    }

    #[tokio::test]
    async fn apply_event_scene_change() {
        let store = make_store();
        let snap = ObsSnapshot {
            scenes: vec![
                SceneState {
                    name: "A".to_string(),
                    active: true,
                    ..Default::default()
                },
                SceneState {
                    name: "B".to_string(),
                    active: false,
                    ..Default::default()
                },
            ],
            current_scene: Some("A".to_string()),
            ..ObsSnapshot::default()
        };
        store.replace(snap).await;

        store
            .apply_event(ObsEvent::CurrentProgramSceneChanged {
                scene_name: "B".to_string(),
            })
            .await;

        let current = store.read().await;
        assert_eq!(current.current_scene.as_deref(), Some("B"));
        assert!(current.scenes[1].active);
        assert!(!current.scenes[0].active);
    }

    #[tokio::test]
    async fn apply_event_mute_change() {
        let store = make_store();
        let snap = ObsSnapshot {
            audio_inputs: vec![AudioState {
                name: "Mic".to_string(),
                muted: Some(false),
                ..Default::default()
            }],
            ..ObsSnapshot::default()
        };
        store.replace(snap).await;

        store
            .apply_event(ObsEvent::InputMuteStateChanged {
                input_name: "Mic".to_string(),
                muted: true,
            })
            .await;

        let current = store.read().await;
        assert_eq!(current.audio_inputs[0].muted, Some(true));
    }

    #[tokio::test]
    async fn update_stats_sets_metrics_and_broadcasts() {
        let store = make_store();
        let mut rx = store.hub.subscribe_state();

        let stats = ObsStats {
            cpu_usage_percent: 42.0,
            active_fps: 60.0,
            ..ObsStats::default()
        };
        store
            .update_stats(stats, Some(4500.0), Some(12_000), Some(3_000))
            .await;

        let current = store.read().await;
        assert_eq!(current.stats, Some(stats));
        assert_eq!(current.stream_bitrate_kbps, Some(4500.0));
        assert_eq!(current.stream_duration_ms, Some(12_000));
        assert_eq!(current.record_duration_ms, Some(3_000));

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");
        match msg {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, Topic::State);
                assert_eq!(data["stream_bitrate_kbps"], 4500.0);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
