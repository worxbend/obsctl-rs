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

    /// Overwrite the entire snapshot and broadcast.
    ///
    /// This discards every field, including the ones the stats poller owns, so
    /// it is only appropriate for seeding a store that has no other writer yet
    /// (daemon startup, tests). The live refresh path must use
    /// [`apply_full_refresh`](Self::apply_full_refresh), which merges instead.
    pub async fn replace(&self, snapshot: ObsSnapshot) {
        let mut guard = self.inner.write().await;
        *guard = snapshot;
        Self::broadcast(&self.hub, &guard);
    }

    /// Fold everything a full OBS refresh learned into the cached snapshot and
    /// broadcast the result.
    ///
    /// The refresh is the authority on scenes, inputs, profiles, collections,
    /// and output flags, but it knows nothing about the metrics the stats
    /// poller writes on its own schedule, so those are carried over rather than
    /// reset. Doing the merge here — under one write lock — is what keeps a
    /// slow refresh (several obs-websocket round-trips) from overwriting events
    /// that arrived while it was still fetching.
    pub async fn apply_full_refresh(
        &self,
        refreshed: &RefreshedObsState,
        scene_cfgs: &[SceneConfig],
        audio_cfgs: &[AudioInputConfig],
    ) {
        let mut snapshot = build_snapshot(refreshed, scene_cfgs, audio_cfgs);

        let mut guard = self.inner.write().await;
        snapshot.stats = guard.stats;
        snapshot.stream_bitrate_kbps = guard.stream_bitrate_kbps;
        snapshot.stream_duration_ms = guard.stream_duration_ms;
        snapshot.record_duration_ms = guard.record_duration_ms;
        *guard = snapshot;
        Self::broadcast(&self.hub, &guard);
    }

    /// Mark OBS as disconnected, record error, and broadcast.
    pub async fn mark_disconnected(&self, error: Option<String>) {
        let mut guard = self.inner.write().await;
        guard.connected = false;
        guard.last_error = error;
        guard.updated_at = OffsetDateTime::now_utc();
        Self::broadcast(&self.hub, &guard);
    }

    /// Apply a single OBS event to the cached snapshot and broadcast.
    pub async fn apply_event(&self, event: ObsEvent) {
        let mut guard = self.inner.write().await;
        if apply_to_snapshot(&mut guard, event) {
            Self::broadcast(&self.hub, &guard);
        }
    }

    /// Publish `snapshot` on the `state` topic.
    ///
    /// Callers hold the write guard across this so that the order subscribers
    /// observe matches the order the writes happened; publishing is a
    /// non-blocking send to the broadcast channel, so nothing awaits under the
    /// lock.
    fn broadcast(hub: &BroadcastHub, snapshot: &ObsSnapshot) {
        let data = match serde_json::to_value(snapshot) {
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
        hub.publish(Topic::State, msg);
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
        Self::broadcast(&self.hub, &guard);
    }

    /// Merge scene and audio config metadata into the snapshot and broadcast.
    ///
    /// Called after config load/reload to attach aliases/shortcuts/groups, so
    /// subscribers need the new metadata pushed to them the same way every
    /// other mutation is pushed.
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
        Self::broadcast(&self.hub, &guard);
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
        // The supervisor answers these with a full refresh, which broadcasts
        // the new snapshot itself. Reporting "changed" here would publish a
        // byte-identical snapshot before the refresh has fetched anything.
        ObsEvent::SceneListChanged => false,
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
        ObsEvent::CurrentProfileChanged { profile_name } => {
            if snapshot.current_profile.as_deref() == Some(&profile_name) {
                return false;
            }
            snapshot.current_profile = Some(profile_name);
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        ObsEvent::ProfileListChanged => false,
        ObsEvent::CurrentSceneCollectionChanged {
            scene_collection_name,
        } => {
            if snapshot.current_scene_collection.as_deref() == Some(&scene_collection_name) {
                return false;
            }
            snapshot.current_scene_collection = Some(scene_collection_name);
            snapshot.updated_at = OffsetDateTime::now_utc();
            true
        }
        ObsEvent::SceneCollectionListChanged => false,
        // High-frequency; don't update the snapshot or broadcast.
        ObsEvent::InputVolumeMeters { .. } => false,
        ObsEvent::Other { .. } => false,
    }
}

/// The obs-websocket versions reported during the handshake.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObsVersions {
    pub studio: String,
    pub websocket: String,
}

/// A set of things OBS can switch between, plus whichever one is currently
/// selected. Scenes, profiles, and scene collections all have this shape, and
/// keeping them as one type stops the three "current" strings from being
/// interchangeable at a call site.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub names: Vec<String>,
    /// Empty when OBS did not report a selection; becomes `None` in the snapshot.
    pub current: String,
}

impl Listing {
    pub fn new(names: Vec<String>, current: impl Into<String>) -> Self {
        Self {
            names,
            current: current.into(),
        }
    }

    fn current_or_none(&self) -> Option<String> {
        (!self.current.is_empty()).then(|| self.current.clone())
    }
}

/// One audio input as OBS reports it, before config metadata is attached.
/// `volume_mul` is a linear multiplier (obs-websocket's `inputVolumeMul`), not
/// a percentage or a dB value — the snapshot derives both of those from it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawInputState {
    pub name: String,
    pub muted: Option<bool>,
    pub volume_mul: Option<f64>,
}

/// Whether the stream and recording outputs are running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputFlags {
    pub streaming: bool,
    pub recording: bool,
}

/// Everything one full OBS refresh learns from obs-websocket.
///
/// Deliberately does not carry the live metrics (`stats`, bitrate, durations):
/// those belong to the stats poller, and
/// [`StateStore::apply_full_refresh`] preserves them rather than asking each
/// caller to remember to copy them across.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefreshedObsState {
    pub versions: ObsVersions,
    pub scenes: Listing,
    pub inputs: Vec<RawInputState>,
    pub profiles: Listing,
    pub collections: Listing,
    pub outputs: OutputFlags,
}

/// Build a full `ObsSnapshot` from a refresh plus the config metadata
/// (aliases, shortcuts, groups) that OBS itself knows nothing about.
pub fn build_snapshot(
    refreshed: &RefreshedObsState,
    scene_cfgs: &[SceneConfig],
    audio_cfgs: &[AudioInputConfig],
) -> ObsSnapshot {
    let current_scene = &refreshed.scenes.current;

    let scenes: Vec<SceneState> = refreshed
        .scenes
        .names
        .iter()
        .map(|name| {
            let cfg = scene_cfgs.iter().find(|c| c.name == *name);
            SceneState {
                name: name.clone(),
                alias: cfg.and_then(|c| c.alias.clone()),
                shortcut: cfg.and_then(|c| c.shortcut.clone()),
                group: cfg.and_then(|c| c.group.clone()),
                active: name == current_scene,
                hidden: cfg.map(|c| c.hidden).unwrap_or(false),
            }
        })
        .collect();

    let audio_inputs: Vec<AudioState> = refreshed
        .inputs
        .iter()
        .map(|input| {
            let cfg = audio_cfgs.iter().find(|c| c.name == input.name);
            AudioState {
                name: input.name.clone(),
                alias: cfg.and_then(|c| c.alias.clone()),
                shortcut: cfg.and_then(|c| c.shortcut.clone()),
                kind: None,
                muted: input.muted,
                volume_mul: input.volume_mul,
                volume_db: input.volume_mul.map(mul_to_db),
                volume_percent: input.volume_mul.map(mul_to_percent),
            }
        })
        .collect();

    ObsSnapshot {
        connected: true,
        obs_studio_version: Some(refreshed.versions.studio.clone()),
        obs_websocket_version: Some(refreshed.versions.websocket.clone()),
        current_scene: Some(current_scene.clone()),
        scenes,
        audio_inputs,
        streaming: refreshed.outputs.streaming,
        recording: refreshed.outputs.recording,
        profiles: refreshed.profiles.names.clone(),
        current_profile: refreshed.profiles.current_or_none(),
        scene_collections: refreshed.collections.names.clone(),
        current_scene_collection: refreshed.collections.current_or_none(),
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
    async fn apply_event_profile_change() {
        let store = make_store();
        let snap = ObsSnapshot {
            current_profile: Some("Default".to_string()),
            ..ObsSnapshot::default()
        };
        store.replace(snap).await;

        store
            .apply_event(ObsEvent::CurrentProfileChanged {
                profile_name: "Streaming".to_string(),
            })
            .await;

        let current = store.read().await;
        assert_eq!(current.current_profile.as_deref(), Some("Streaming"));
    }

    #[test]
    fn build_snapshot_populates_profiles() {
        let snapshot = build_snapshot(
            &RefreshedObsState {
                versions: ObsVersions {
                    studio: "30.1.0".to_string(),
                    websocket: "5.3.0".to_string(),
                },
                profiles: Listing::new(
                    vec!["Default".to_string(), "Streaming".to_string()],
                    "Streaming",
                ),
                collections: Listing::new(
                    vec!["Podcast".to_string(), "Gaming".to_string()],
                    "Gaming",
                ),
                ..RefreshedObsState::default()
            },
            &[],
            &[],
        );
        assert_eq!(snapshot.profiles, vec!["Default", "Streaming"]);
        assert_eq!(snapshot.current_profile.as_deref(), Some("Streaming"));
        assert_eq!(snapshot.scene_collections, vec!["Podcast", "Gaming"]);
        assert_eq!(snapshot.current_scene_collection.as_deref(), Some("Gaming"));
    }

    /// A full refresh is the authority on scenes and inputs, but the stats
    /// poller owns the live metrics on its own schedule. Before the store did
    /// this merge itself, every caller had to hand-copy those four fields, and
    /// forgetting one flickered the live bar back to "waiting".
    #[tokio::test]
    async fn full_refresh_keeps_poller_owned_metrics() {
        let store = make_store();
        store
            .update_stats(ObsStats::default(), Some(4500.0), Some(12_000), Some(3_000))
            .await;

        store
            .apply_full_refresh(
                &RefreshedObsState {
                    scenes: Listing::new(vec!["Main".to_string()], "Main"),
                    ..RefreshedObsState::default()
                },
                &[],
                &[],
            )
            .await;

        let current = store.read().await;
        assert_eq!(current.current_scene.as_deref(), Some("Main"));
        assert_eq!(current.stream_bitrate_kbps, Some(4500.0));
        assert_eq!(current.stream_duration_ms, Some(12_000));
        assert_eq!(current.record_duration_ms, Some(3_000));
        assert!(current.stats.is_some());
    }

    /// Config reload attaches aliases/shortcuts to the cached snapshot, and
    /// subscribed clients have to be told — otherwise the TUI keeps rendering
    /// the old names until something unrelated happens to broadcast.
    #[tokio::test]
    async fn merge_config_broadcasts_updated_metadata() {
        let store = make_store();
        store
            .replace(ObsSnapshot {
                scenes: vec![SceneState {
                    name: "Main".to_string(),
                    ..Default::default()
                }],
                ..ObsSnapshot::default()
            })
            .await;

        let mut rx = store.hub.subscribe_state();
        store
            .merge_config(
                &[SceneConfig {
                    name: "Main".to_string(),
                    alias: Some("m".to_string()),
                    ..Default::default()
                }],
                &[],
            )
            .await;

        let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");
        match msg {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, Topic::State);
                assert_eq!(data["scenes"][0]["alias"], "m");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    /// The supervisor answers list-changed events with a full refresh, which
    /// broadcasts on its own. Broadcasting here too would push an unchanged
    /// snapshot — the false positive `CLAUDE.md` warns about.
    #[tokio::test]
    async fn list_changed_events_do_not_broadcast_unchanged_state() {
        let store = make_store();
        let mut rx = store.hub.subscribe_state();

        for event in [
            ObsEvent::SceneListChanged,
            ObsEvent::ProfileListChanged,
            ObsEvent::SceneCollectionListChanged,
        ] {
            store.apply_event(event).await;
        }

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
                .await
                .is_err(),
            "list-changed events should not broadcast on their own"
        );
    }

    #[tokio::test]
    async fn apply_event_scene_collection_change() {
        let store = make_store();
        let snap = ObsSnapshot {
            current_scene_collection: Some("Podcast".to_string()),
            ..ObsSnapshot::default()
        };
        store.replace(snap).await;

        store
            .apply_event(ObsEvent::CurrentSceneCollectionChanged {
                scene_collection_name: "Gaming".to_string(),
            })
            .await;

        let current = store.read().await;
        assert_eq!(current.current_scene_collection.as_deref(), Some("Gaming"));
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
