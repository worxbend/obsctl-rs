use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use time::OffsetDateTime;
use tokio::sync::{RwLock, RwLockWriteGuard};
use tracing::debug;

use crate::config::model::{AudioInputConfig, SceneConfig};
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
    /// Counts events that changed the snapshot. See [`StateStore::generation`].
    generation: Arc<AtomicU64>,
}

impl StateStore {
    pub fn new(hub: Arc<BroadcastHub>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ObsSnapshot::default())),
            hub,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// How many times the snapshot has been changed by something other than a
    /// full refresh.
    ///
    /// A full refresh takes a dozen obs-websocket round-trips and builds its
    /// result from replies gathered over all of them, so by the time it is
    /// ready to publish, the state it describes may already be out of date.
    /// Reading this before the fetch and again after tells the refresher
    /// whether anything it is about to overwrite arrived in the meantime.
    ///
    /// Not part of `ObsSnapshot`, so it is not a wire-protocol change.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Record that a write happened: count it, then publish the result.
    ///
    /// Every mutation a slow full refresh can silently undo goes through here,
    /// so that the refresh has one number to compare against rather than a
    /// per-method guess about which writes matter. It used to be only
    /// [`apply_event`](Self::apply_event) that counted, which meant a refresh
    /// finishing just after the connection dropped overwrote
    /// [`mark_disconnected`](Self::mark_disconnected)'s work — republishing
    /// `connected: true` with no error for a dead connection — and then
    /// reported that nothing had overtaken it, so nobody fetched again.
    ///
    /// [`update_stats`](Self::update_stats) deliberately does not call this:
    /// the fields it writes are carried across a refresh by [`PolledMetrics`],
    /// so a refresh cannot clobber them and there is nothing to re-fetch.
    ///
    /// The counter is bumped while the write guard is still held, so a refresh
    /// comparing generations either sees this write in the snapshot it is
    /// replacing or sees the bump. It cannot miss both.
    fn commit(&self, guard: &mut RwLockWriteGuard<'_, ObsSnapshot>) {
        self.generation.fetch_add(1, Ordering::Release);
        Self::broadcast(&self.hub, guard);
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
        self.commit(&mut guard);
    }

    /// Replace the snapshot with one built from a full OBS fetch.
    ///
    /// `generation_before` is [`StateStore::generation`] as it was when the
    /// fetch started. The return value is `true` when an event changed the
    /// snapshot while the fetch was in flight, which means this (older) result
    /// has just overwritten it and the caller should fetch again.
    ///
    /// Taking the write lock does not prevent that. It makes the swap atomic,
    /// which is a different thing: an event applied at any point between the
    /// first round-trip and this call is simply not represented in `refreshed`.
    ///
    /// One group of fields is carried across rather than replaced: the metrics
    /// the stats poller owns (see [`PolledMetrics`]). A refresh asks OBS about
    /// scenes, inputs and outputs and never about live statistics, so building
    /// the new snapshot leaves those fields at their defaults; copying the
    /// existing values in before the swap is what stops a refresh from blanking
    /// the live bitrate and durations mid-stream.
    #[must_use = "a superseded refresh leaves stale values in the snapshot until another one runs"]
    pub async fn apply_full_refresh(
        &self,
        refreshed: &RefreshedObsState,
        scene_cfgs: &[SceneConfig],
        audio_cfgs: &[AudioInputConfig],
        generation_before: u64,
    ) -> bool {
        let mut snapshot = build_snapshot(refreshed, scene_cfgs, audio_cfgs);

        let mut guard = self.inner.write().await;
        // Metrics belong to the stats poller, not to this fetch; carry the
        // ones already in the snapshot across rather than blanking them.
        PolledMetrics::read_from(&guard).write_into(&mut snapshot);
        *guard = snapshot;
        Self::broadcast(&self.hub, &guard);
        drop(guard);

        self.generation() != generation_before
    }

    /// Record that OBS is reachable, along with the versions the handshake
    /// reported, and broadcast.
    ///
    /// The normal way the snapshot becomes `connected` is
    /// [`apply_full_refresh`](Self::apply_full_refresh), which sets the flag as
    /// part of publishing everything it read. This is for the case where that
    /// read failed but the connection itself is fine: the supervisor still
    /// installs the client, so `CommandExecutor::require_obs` hands it to
    /// `scene`, `mute` and `vol` and they work — and a snapshot left saying
    /// `connected: false` would have `obs-status` and the TUI reporting the
    /// opposite of what those commands do.
    ///
    /// `error` is the reason the snapshot is incomplete, so a client can see
    /// why the scene and input lists are missing or older than the connection.
    /// Those lists are left as they are rather than blanked, matching what
    /// [`mark_disconnected`](Self::mark_disconnected) does: last-known is more
    /// use to a reader than empty.
    pub async fn mark_connected(&self, versions: &ObsVersions, error: Option<String>) {
        let mut guard = self.inner.write().await;
        guard.connected = true;
        guard.obs_studio_version = Some(versions.studio.clone());
        guard.obs_websocket_version = Some(versions.websocket.clone());
        guard.last_error = error;
        guard.updated_at = OffsetDateTime::now_utc();
        self.commit(&mut guard);
    }

    /// Mark OBS as disconnected, record error, and broadcast.
    pub async fn mark_disconnected(&self, error: Option<String>) {
        let mut guard = self.inner.write().await;
        guard.connected = false;
        guard.last_error = error;
        guard.updated_at = OffsetDateTime::now_utc();
        self.commit(&mut guard);
    }

    /// Apply a single OBS event to the cached snapshot and broadcast.
    pub async fn apply_event(&self, event: ObsEvent) {
        let mut guard = self.inner.write().await;
        if apply_to_snapshot(&mut guard, event) {
            self.commit(&mut guard);
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
    ///
    /// Broadcasts without going through [`commit`](Self::commit) on purpose —
    /// see the note there about why these fields are outside the staleness
    /// fence.
    pub async fn update_stats(&self, metrics: PolledMetrics) {
        let mut guard = self.inner.write().await;
        metrics.write_into(&mut guard);
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
        for scene in guard.scenes.iter_mut() {
            let cfg = scenes.iter().find(|c| c.name == scene.name);
            apply_scene_config(scene, cfg);
        }
        for input in guard.audio_inputs.iter_mut() {
            let cfg = audio_inputs.iter().find(|c| c.name == input.name);
            apply_audio_config(input, cfg);
        }
        guard.updated_at = OffsetDateTime::now_utc();
        self.commit(&mut guard);
    }
}

/// The snapshot fields the stats poller owns, as one value.
///
/// They are written by `spawn_stats_poller` on its own schedule and must
/// survive a full refresh, which knows nothing about them. Before this type
/// they were four fields listed by hand in `update_stats` and again in
/// `apply_full_refresh`; that has already gone wrong once (9a22423 added the
/// carry-over because a refresh was blanking them mid-stream), and the two
/// `Option<u64>` durations sat adjacent in a positional argument list where
/// swapping them would have compiled.
///
/// `read_from` is written as an exhaustive struct literal on purpose: adding a
/// field to this type without teaching it how to be read is a compile error
/// there rather than a metric that silently stops surviving refreshes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PolledMetrics {
    pub stats: Option<ObsStats>,
    pub stream_bitrate_kbps: Option<f64>,
    pub stream_duration_ms: Option<u64>,
    pub record_duration_ms: Option<u64>,
}

impl PolledMetrics {
    /// A fresh reading. `stats` is not optional here: the poller only reports
    /// when it has one, so there is no way to pass `None` by accident.
    pub fn new(
        stats: ObsStats,
        stream_bitrate_kbps: Option<f64>,
        stream_duration_ms: Option<u64>,
        record_duration_ms: Option<u64>,
    ) -> Self {
        Self {
            stats: Some(stats),
            stream_bitrate_kbps,
            stream_duration_ms,
            record_duration_ms,
        }
    }

    fn read_from(snapshot: &ObsSnapshot) -> Self {
        Self {
            stats: snapshot.stats,
            stream_bitrate_kbps: snapshot.stream_bitrate_kbps,
            stream_duration_ms: snapshot.stream_duration_ms,
            record_duration_ms: snapshot.record_duration_ms,
        }
    }

    fn write_into(self, snapshot: &mut ObsSnapshot) {
        snapshot.stats = self.stats;
        snapshot.stream_bitrate_kbps = self.stream_bitrate_kbps;
        snapshot.stream_duration_ms = self.stream_duration_ms;
        snapshot.record_duration_ms = self.record_duration_ms;
    }
}

/// Project a scene's config entry onto its snapshot state.
///
/// `None` means the config does not mention this scene, and must clear the
/// metadata rather than leave it: an alias the user deleted has to stop
/// resolving, and the alias tables the executor searches are built from this
/// snapshot. Both the full-refresh path (`build_snapshot`) and the reload path
/// (`merge_config`) go through here so they cannot disagree about that.
///
/// Only config-derived fields are touched. `active` belongs to OBS.
fn apply_scene_config(scene: &mut SceneState, cfg: Option<&SceneConfig>) {
    scene.alias = cfg.and_then(|c| c.alias.clone());
    scene.shortcut = cfg.and_then(|c| c.shortcut.clone());
    scene.group = cfg.and_then(|c| c.group.clone());
    scene.hidden = cfg.is_some_and(|c| c.hidden);
}

/// Project an audio input's config entry onto its snapshot state, with the
/// same rule as [`apply_scene_config`]. `muted`, the volumes, and `kind`
/// belong to OBS and are left alone.
fn apply_audio_config(input: &mut AudioState, cfg: Option<&AudioInputConfig>) {
    input.alias = cfg.and_then(|c| c.alias.clone());
    input.shortcut = cfg.and_then(|c| c.shortcut.clone());
}

/// Fold one OBS event into the cached snapshot, reporting whether anything
/// actually changed.
///
/// `false` means "identical snapshot" and stops a redundant broadcast: OBS
/// re-announces state the daemon already holds (a scene switched to the one
/// already live, a mute set to the value already set), and every client is
/// woken by a broadcast.
///
/// The `updated_at` stamp is applied here, once, rather than in each arm of
/// [`mutate_snapshot`]. It used to be eight hand-written copies of the same
/// line, so a new event handler was one forgotten line away from changing the
/// snapshot while leaving it claiming it had not changed.
fn apply_to_snapshot(snapshot: &mut ObsSnapshot, event: ObsEvent) -> bool {
    let changed = mutate_snapshot(snapshot, event);
    if changed {
        snapshot.updated_at = OffsetDateTime::now_utc();
    }
    changed
}

/// Apply `event` to `snapshot`, answering only "did this change anything?".
/// Callers get the timestamp handling from [`apply_to_snapshot`].
fn mutate_snapshot(snapshot: &mut ObsSnapshot, event: ObsEvent) -> bool {
    match event {
        ObsEvent::CurrentProgramSceneChanged { scene_name } => {
            if snapshot.current_scene.as_deref() == Some(&scene_name) {
                return false;
            }
            for s in snapshot.scenes.iter_mut() {
                s.active = s.name == scene_name;
            }
            snapshot.current_scene = Some(scene_name);
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
            true
        }
        ObsEvent::InputRemoved { input_name } => {
            let before = snapshot.audio_inputs.len();
            snapshot.audio_inputs.retain(|a| a.name != input_name);
            snapshot.audio_inputs.len() != before
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
                // OBS reported both, and its dB is rounded rather than
                // exactly `mul_to_db` of its multiplier; keep what it sent.
                a.set_level_with_db(volume_mul, volume_db);
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
            true
        }
        ObsEvent::RecordStateChanged { active } => {
            if snapshot.recording == active {
                return false;
            }
            snapshot.recording = active;
            true
        }
        ObsEvent::CurrentProfileChanged { profile_name } => {
            if snapshot.current_profile.as_deref() == Some(&profile_name) {
                return false;
            }
            snapshot.current_profile = Some(profile_name);
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
            let mut scene = SceneState {
                name: name.clone(),
                active: name == current_scene,
                ..SceneState::default()
            };
            apply_scene_config(&mut scene, scene_cfgs.iter().find(|c| c.name == *name));
            scene
        })
        .collect();

    let audio_inputs: Vec<AudioState> = refreshed
        .inputs
        .iter()
        .map(|input| {
            let mut state = AudioState {
                name: input.name.clone(),
                kind: None,
                muted: input.muted,
                ..AudioState::default()
            };
            match input.volume_mul {
                Some(volume_mul) => state.set_level(volume_mul),
                None => state.clear_level(),
            }
            apply_audio_config(&mut state, audio_cfgs.iter().find(|c| c.name == input.name));
            state
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

    /// The supervisor installs the OBS client even when the connect-path
    /// snapshot fetch fails, so the daemon can serve `scene`/`mute`/`vol`
    /// against a connection whose scene list it never managed to read. The
    /// snapshot has to say so, or `obs-status` reports "disconnected" while
    /// those same commands succeed.
    #[tokio::test]
    async fn mark_connected_records_the_connection_and_why_the_snapshot_is_thin() {
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

        store
            .mark_connected(
                &ObsVersions {
                    studio: "30.1.0".to_string(),
                    websocket: "5.3.0".to_string(),
                },
                Some("initial snapshot fetch failed: timeout".to_string()),
            )
            .await;

        let current = store.read().await;
        assert!(current.connected);
        assert_eq!(current.obs_studio_version.as_deref(), Some("30.1.0"));
        assert_eq!(current.obs_websocket_version.as_deref(), Some("5.3.0"));
        assert_eq!(
            current.last_error.as_deref(),
            Some("initial snapshot fetch failed: timeout")
        );
        // Last-known lists survive; blanking them would replace "possibly
        // stale" with "definitely nothing".
        assert_eq!(current.scenes.len(), 1);
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

    /// Every event that changes the snapshot must also move `updated_at`, and
    /// every event that changes nothing must leave it alone. Each match arm
    /// used to stamp the time itself, so this pins the behaviour now that
    /// `apply_to_snapshot` does it in one place for all of them.
    #[tokio::test]
    async fn changing_events_advance_updated_at_and_no_ops_do_not() {
        let store = make_store();
        store
            .replace(ObsSnapshot {
                streaming: false,
                ..ObsSnapshot::default()
            })
            .await;
        let before = store.read().await.updated_at;

        store
            .apply_event(ObsEvent::StreamStateChanged { active: true })
            .await;
        let after_change = store.read().await;
        assert!(after_change.streaming);
        assert!(
            after_change.updated_at > before,
            "a real change must move the timestamp"
        );

        // OBS re-announcing the state we already hold is not a change.
        store
            .apply_event(ObsEvent::StreamStateChanged { active: true })
            .await;
        assert_eq!(
            store.read().await.updated_at,
            after_change.updated_at,
            "a no-op event must leave the timestamp alone"
        );
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
            .update_stats(PolledMetrics::new(
                ObsStats::default(),
                Some(4500.0),
                Some(12_000),
                Some(3_000),
            ))
            .await;

        let superseded = store
            .apply_full_refresh(
                &RefreshedObsState {
                    scenes: Listing::new(vec!["Main".to_string()], "Main"),
                    ..RefreshedObsState::default()
                },
                &[],
                &[],
                store.generation(),
            )
            .await;
        assert!(!superseded, "no events landed during this refresh");

        let current = store.read().await;
        assert_eq!(current.current_scene.as_deref(), Some("Main"));
        assert_eq!(current.stream_bitrate_kbps, Some(4500.0));
        assert_eq!(current.stream_duration_ms, Some(12_000));
        assert_eq!(current.record_duration_ms, Some(3_000));
        assert!(current.stats.is_some());
    }

    /// An event that lands while a refresh is in flight is reported, so the
    /// refresher can fetch again instead of leaving the older value in place.
    ///
    /// The write lock makes the swap atomic; it does not make the fetch
    /// current. This is what the two comments claiming otherwise used to say.
    #[tokio::test]
    async fn a_refresh_overtaken_by_an_event_reports_that_it_is_stale() {
        let store = make_store();
        store
            .replace(ObsSnapshot {
                scenes: vec![SceneState {
                    name: "Main".to_string(),
                    active: true,
                    ..Default::default()
                }],
                current_scene: Some("Main".to_string()),
                ..ObsSnapshot::default()
            })
            .await;

        // The fetch starts here, and describes OBS as it was at this moment.
        let generation_before = store.generation();

        // ...and while its round-trips are in flight, the scene changes.
        store
            .apply_event(ObsEvent::CurrentProgramSceneChanged {
                scene_name: "Second".to_string(),
            })
            .await;

        let superseded = store
            .apply_full_refresh(
                &RefreshedObsState {
                    scenes: Listing::new(vec!["Main".to_string(), "Second".to_string()], "Main"),
                    ..RefreshedObsState::default()
                },
                &[],
                &[],
                generation_before,
            )
            .await;

        // The stale fetch did overwrite the event — that part is unavoidable,
        // its data predates it — but it says so rather than leaving the wrong
        // scene showing until the next event of that kind.
        assert_eq!(store.read().await.current_scene.as_deref(), Some("Main"));
        assert!(
            superseded,
            "a refresh overtaken by an event must ask to be run again"
        );
    }

    /// A refresh can be slower than the connection it is reading from. All its
    /// obs-websocket round-trips can be finished while the socket is closing,
    /// so `mark_disconnected` runs first and the refresh publishes afterwards —
    /// and the snapshot a refresh builds always says `connected: true` and
    /// carries no `last_error`, because it describes an OBS that was answering.
    ///
    /// The overwrite itself is unavoidable; being quiet about it is not. Until
    /// `mark_disconnected` counted as a write, the refresh reported "nothing
    /// overtook me", nobody fetched again, and the daemon sat there telling
    /// every client OBS was connected when it was not.
    #[tokio::test]
    async fn a_refresh_that_lands_after_a_disconnect_reports_that_it_is_stale() {
        let store = make_store();
        store
            .replace(ObsSnapshot {
                connected: true,
                ..ObsSnapshot::default()
            })
            .await;

        // The fetch starts here, against a connection that is still up.
        let generation_before = store.generation();

        // ...and the socket closes while its round-trips are in flight.
        store
            .mark_disconnected(Some("OBS disconnected".to_string()))
            .await;

        let superseded = store
            .apply_full_refresh(&RefreshedObsState::default(), &[], &[], generation_before)
            .await;

        assert!(
            superseded,
            "a refresh that overwrote a disconnect must ask to be run again"
        );
        // Showing what it overwrote, so the reason for the re-fetch is on the
        // record: the daemon is now claiming a connection that is gone.
        let current = store.read().await;
        assert!(current.connected);
        assert_eq!(current.last_error, None);
    }

    /// Config reload is the other write a slow refresh can undo: the aliases it
    /// attaches are not in the snapshot the refresh built, so the refresh has
    /// to be told to run again and pick them up.
    #[tokio::test]
    async fn a_refresh_that_lands_after_a_config_merge_reports_that_it_is_stale() {
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

        let generation_before = store.generation();
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

        let superseded = store
            .apply_full_refresh(
                &RefreshedObsState {
                    scenes: Listing::new(vec!["Main".to_string()], "Main"),
                    ..RefreshedObsState::default()
                },
                &[],
                &[],
                generation_before,
            )
            .await;

        assert!(superseded);
    }

    /// Events that change nothing do not force a re-refresh: OBS re-announces
    /// state the daemon already holds, and treating that as a reason to fetch
    /// again would put the refresher in a loop.
    #[tokio::test]
    async fn a_no_op_event_does_not_make_a_refresh_look_stale() {
        let store = make_store();
        store
            .replace(ObsSnapshot {
                streaming: true,
                ..ObsSnapshot::default()
            })
            .await;

        let generation_before = store.generation();
        store
            .apply_event(ObsEvent::StreamStateChanged { active: true })
            .await;

        let superseded = store
            .apply_full_refresh(&RefreshedObsState::default(), &[], &[], generation_before)
            .await;
        assert!(!superseded);
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

    /// A scene or input the reloaded config no longer mentions loses the
    /// metadata that config gave it — the same rule `build_snapshot` applies
    /// on the full-refresh path.
    ///
    /// This is what makes `reload-config` able to *remove* an alias. The alias
    /// tables the executor searches are built from this snapshot, so metadata
    /// left behind here keeps a deleted alias resolving.
    #[tokio::test]
    async fn merge_config_clears_metadata_for_entries_the_config_dropped() {
        let store = make_store();
        store
            .replace(ObsSnapshot {
                scenes: vec![SceneState {
                    name: "Main".to_string(),
                    alias: Some("cam".to_string()),
                    shortcut: Some("m".to_string()),
                    group: Some("live".to_string()),
                    hidden: true,
                    ..Default::default()
                }],
                audio_inputs: vec![AudioState {
                    name: "Mic".to_string(),
                    alias: Some("mic".to_string()),
                    shortcut: Some("M".to_string()),
                    ..Default::default()
                }],
                ..ObsSnapshot::default()
            })
            .await;

        // A reloaded config that no longer mentions either of them.
        store.merge_config(&[], &[]).await;

        let snap = store.read().await;
        assert_eq!(snap.scenes[0].alias, None);
        assert_eq!(snap.scenes[0].shortcut, None);
        assert_eq!(snap.scenes[0].group, None);
        assert!(!snap.scenes[0].hidden);
        assert_eq!(snap.audio_inputs[0].alias, None);
        assert_eq!(snap.audio_inputs[0].shortcut, None);
        // The name and OBS-owned state survive; only config metadata is cleared.
        assert_eq!(snap.scenes[0].name, "Main");
        assert_eq!(snap.audio_inputs[0].name, "Mic");
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
            .update_stats(PolledMetrics::new(
                stats,
                Some(4500.0),
                Some(12_000),
                Some(3_000),
            ))
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
