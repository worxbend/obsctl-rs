use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, info, warn};

use crate::config::model::Config;
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::ipc::{
    protocol::{LogEvent, LogLevel},
    session::BroadcastHub,
};
use crate::obs::{
    client::{ObsClient, ObsEvent},
    connection::{ObsConnectionParams, connect},
    requests,
    state::ObsStats,
    validation::extract_resource_names,
};
use crate::runtime::reconnect_policy::ReconnectPolicy;
use crate::server::obs_event_adapter::normalize_obs_event;
use crate::server::state_store::{
    Listing, ObsVersions, OutputFlags, RawInputState, RefreshedObsState, StateStore,
};
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// How many times the connect-path snapshot fetch will retry when events keep
/// landing mid-fetch before settling for the result it has.
const MAX_REFRESH_RETRIES: usize = 3;

pub struct ObsSupervisor {
    config: Arc<Mutex<Config>>,
    state: StateStore,
    obs_handle: Arc<Mutex<Option<ObsClient>>>,
    reconnecting: Arc<AtomicBool>,
    reconnect_rx: mpsc::Receiver<()>,
    shutdown: watch::Receiver<bool>,
    hub: Arc<BroadcastHub>,
}

/// Everything the supervisor needs to be handed at startup.
///
/// A struct rather than seven positional parameters, matching
/// `CommandExecutorConfig` next to it in the daemon's wiring: the daemon
/// constructs both one after the other, and reading two adjacent call sites
/// written in two different styles is harder than it needs to be. It also
/// means each value is named where it is passed.
pub struct ObsSupervisorConfig {
    pub config: Arc<Mutex<Config>>,
    pub state: StateStore,
    pub obs_handle: Arc<Mutex<Option<ObsClient>>>,
    pub reconnecting: Arc<AtomicBool>,
    pub reconnect_rx: mpsc::Receiver<()>,
    pub shutdown: watch::Receiver<bool>,
    pub hub: Arc<BroadcastHub>,
}

impl ObsSupervisor {
    pub fn new(cfg: ObsSupervisorConfig) -> Self {
        Self {
            config: cfg.config,
            state: cfg.state,
            obs_handle: cfg.obs_handle,
            reconnecting: cfg.reconnecting,
            reconnect_rx: cfg.reconnect_rx,
            shutdown: cfg.shutdown,
            hub: cfg.hub,
        }
    }

    pub async fn run(mut self) {
        let mut reconnect_cfg = {
            let cfg = self.config.lock().await;
            cfg.reconnect.clone()
        };
        let mut policy = ReconnectPolicy::new(reconnect_cfg.clone());

        loop {
            let (params, latest_reconnect_cfg) = {
                let cfg = self.config.lock().await;
                (
                    ObsConnectionParams::from_config(&cfg.connection),
                    cfg.reconnect.clone(),
                )
            };
            let attempt = match params {
                Ok(params) => self.attempt_connect(&params).await,
                Err(error) => Err(error),
            };
            if latest_reconnect_cfg != reconnect_cfg {
                reconnect_cfg = latest_reconnect_cfg;
                policy = ReconnectPolicy::new(reconnect_cfg.clone());
            }

            self.reconnecting.store(true, Ordering::Relaxed);
            match attempt {
                Ok((client, obs_version, ws_version, disconnect_rx)) => {
                    info!("OBS connected: studio={obs_version} ws={ws_version}");
                    self.publish_log(
                        LogLevel::Info,
                        format!("OBS connected: studio={obs_version} ws={ws_version}"),
                    );
                    policy.reset();
                    self.reconnecting.store(false, Ordering::Relaxed);

                    let fetch_result = self
                        .fetch_and_publish_snapshot(&client, &obs_version, &ws_version)
                        .await;
                    if fetch_result.is_err() {
                        warn!("Initial snapshot fetch failed");
                        self.publish_log(LogLevel::Warn, "Initial OBS snapshot fetch failed");
                    }

                    *self.obs_handle.lock().await = Some(client);

                    // Wait until OBS disconnects or reconnect/shutdown is requested
                    let reason = self.wait_for_disconnect(disconnect_rx).await;
                    *self.obs_handle.lock().await = None;

                    match reason {
                        DisconnectReason::Shutdown => {
                            self.publish_log(LogLevel::Info, "OBS disconnected during shutdown");
                            self.state.mark_disconnected(None).await;
                            break;
                        }
                        DisconnectReason::ReconnectRequested => {
                            self.publish_log(
                                LogLevel::Warn,
                                "OBS disconnected: reconnect requested",
                            );
                            self.state
                                .mark_disconnected(Some("reconnect requested".to_string()))
                                .await;
                            continue;
                        }
                        DisconnectReason::ObsDisconnected => {
                            self.publish_log(LogLevel::Warn, "OBS WebSocket closed");
                            self.state
                                .mark_disconnected(Some("OBS disconnected".to_string()))
                                .await;
                            // Fall through to the reconnect delay loop below.
                        }
                    }
                }
                Err(e) => {
                    warn!("OBS connection failed: {e}");
                    self.publish_log(LogLevel::Warn, format!("OBS unavailable: {e}"));
                    self.state.mark_disconnected(Some(e.to_string())).await;
                }
            }

            // Reconnect delay loop
            if !policy.enabled() {
                info!("Reconnect disabled; supervisor stopping");
                self.reconnecting.store(false, Ordering::Relaxed);
                self.publish_log(
                    LogLevel::Info,
                    "OBS reconnect disabled; supervisor stopping",
                );
                break;
            }
            loop {
                let Some(delay) = policy.next_delay() else {
                    info!("Reconnect exhausted; supervisor stopping");
                    self.reconnecting.store(false, Ordering::Relaxed);
                    self.publish_log(
                        LogLevel::Warn,
                        "OBS reconnect exhausted; supervisor stopping",
                    );
                    return;
                };
                info!("Reconnecting in {delay:?}");
                tokio::select! {
                    _ = tokio::time::sleep(delay) => break,
                    _ = self.reconnect_rx.recv() => {
                        info!("Reconnect requested immediately");
                        self.publish_log(LogLevel::Info, "OBS reconnect requested immediately");
                        break;
                    }
                    _ = self.shutdown.changed() => {
                        if *self.shutdown.borrow() {
                            self.reconnecting.store(false, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            }

            if *self.shutdown.borrow() {
                break;
            }
        }
    }

    async fn attempt_connect(
        &self,
        params: &ObsConnectionParams,
    ) -> Result<(
        ObsClient,
        String,
        String,
        tokio::sync::oneshot::Receiver<()>,
    )> {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ObsEvent>(64);
        let (client, obs_version, ws_version, disconnect_rx) = connect(params, event_tx).await?;
        let event_client = client.clone();
        let state = self.state.clone();
        let state_for_refresh = self.state.clone();
        let config = Arc::clone(&self.config);
        let hub = Arc::clone(&self.hub);
        let event_obs_version = obs_version.clone();
        let event_ws_version = ws_version.clone();

        spawn_stats_poller(client.clone(), self.state.clone());

        // Capacity 1, and the event task uses `try_send`: while a refresh is
        // running, a burst of list-changed events coalesces into exactly one
        // follow-up rather than queueing a refresh per event. The refresher
        // exits when the event task drops the sender, so it dies with the
        // connection it belongs to.
        let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel::<()>(1);
        // The refresher's own handle, so a fetch that was overtaken by events
        // can queue a follow-up. `Weak` rather than a clone: an ordinary
        // sender held by this task would keep the channel open forever and the
        // task would never see the event task drop its end.
        let refresh_self_tx = refresh_tx.downgrade();

        let refresh_hub = Arc::clone(&hub);
        tokio::spawn(async move {
            while refresh_rx.recv().await.is_some() {
                match fetch_and_publish_snapshot_from(
                    &config,
                    &state_for_refresh,
                    &event_client,
                    &event_obs_version,
                    &event_ws_version,
                )
                .await
                {
                    // Events arrived while this fetch was in flight, so it
                    // published values that are already out of date. Queue one
                    // more pass; the capacity-1 channel coalesces repeats.
                    Ok(true) => {
                        debug!("OBS events landed during a refresh; queueing another");
                        if let Some(tx) = refresh_self_tx.upgrade() {
                            let _ = tx.try_send(());
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        warn!("OBS snapshot refresh after list change failed: {e}");
                        refresh_hub.publish_log(
                            LogEvent::new(
                                LogLevel::Warn,
                                format!("OBS snapshot refresh after list change failed: {e}"),
                            )
                            .with_target("obsctl_rs::server::obs_supervisor"),
                        );
                    }
                }
            }
        });

        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let needs_full_refresh = matches!(
                    event,
                    ObsEvent::SceneListChanged
                        | ObsEvent::ProfileListChanged
                        | ObsEvent::SceneCollectionListChanged
                );

                // Log notable remote OBS changes before applying them.
                log_obs_event(&hub, &event);
                state.apply_event(event.clone()).await;
                if let Some(payload) = normalize_obs_event(&event) {
                    hub.publish_obs_event(payload);
                }

                // Refreshing inline would block this loop for a dozen-plus
                // obs-websocket round-trips, back-pressuring the bounded event
                // channel and delaying every mute, volume, and scene change
                // queued behind it. A full channel means a refresh is already
                // pending, which is all the signal we need.
                if needs_full_refresh {
                    let _ = refresh_tx.try_send(());
                }
            }
        });

        Ok((client, obs_version, ws_version, disconnect_rx))
    }

    /// Fetch and publish a full snapshot, retrying while events keep landing
    /// mid-fetch.
    ///
    /// Bounded rather than a `while` loop: this runs on the connect path, and
    /// a busy OBS must not be able to keep it fetching instead of getting on
    /// with serving clients. Giving up leaves the same staleness the old code
    /// always had, so the bound costs nothing that was not already the case.
    async fn fetch_and_publish_snapshot(
        &self,
        client: &ObsClient,
        obs_version: &str,
        ws_version: &str,
    ) -> Result<()> {
        for _ in 0..MAX_REFRESH_RETRIES {
            let superseded = fetch_and_publish_snapshot_from(
                &self.config,
                &self.state,
                client,
                obs_version,
                ws_version,
            )
            .await?;
            if !superseded {
                return Ok(());
            }
            debug!("OBS events landed during the snapshot fetch; refreshing again");
        }
        Ok(())
    }

    async fn wait_for_disconnect(
        &mut self,
        mut disconnect: tokio::sync::oneshot::Receiver<()>,
    ) -> DisconnectReason {
        loop {
            tokio::select! {
                _ = &mut disconnect => {
                    return DisconnectReason::ObsDisconnected;
                }
                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        return DisconnectReason::Shutdown;
                    }
                }
                Some(()) = self.reconnect_rx.recv() => {
                    return DisconnectReason::ReconnectRequested;
                }
            }
        }
    }

    fn publish_log(&self, level: LogLevel, message: impl AsRef<str>) {
        self.hub.publish_log(
            LogEvent::new(level, message).with_target("obsctl_rs::server::obs_supervisor"),
        );
    }
}

enum DisconnectReason {
    Shutdown,
    ReconnectRequested,
    ObsDisconnected,
}

async fn fetch_and_publish_snapshot_from(
    config: &Arc<Mutex<Config>>,
    state: &StateStore,
    client: &ObsClient,
    obs_version: &str,
    ws_version: &str,
) -> Result<bool> {
    // Read before the first round-trip: everything below describes OBS as it
    // was at some point after this, and any event that lands in between is
    // information this fetch does not have.
    let generation_before = state.generation();

    let scenes = fetch_scene_listing(client).await?;
    let inputs = fetch_input_states(client).await?;

    let refreshed = RefreshedObsState {
        versions: ObsVersions {
            studio: obs_version.to_string(),
            websocket: ws_version.to_string(),
        },
        scenes,
        inputs,
        profiles: fetch_listing(client, ListingRequest::PROFILES).await,
        collections: fetch_listing(client, ListingRequest::SCENE_COLLECTIONS).await,
        outputs: OutputFlags {
            streaming: fetch_output_active(
                client,
                requests::get_stream_status(),
                "GetStreamStatus",
            )
            .await,
            recording: fetch_output_active(
                client,
                requests::get_record_status(),
                "GetRecordStatus",
            )
            .await,
        },
    };

    let (scene_cfgs, audio_cfgs) = {
        let cfg = config.lock().await;
        (cfg.scenes.clone(), cfg.audio.inputs.clone())
    };

    // The write lock keeps the swap atomic and preserves the metrics the stats
    // poller owns. It cannot keep this fetch current: an event applied while
    // the round-trips above were running is not in `refreshed` and has just
    // been overwritten. `apply_full_refresh` reports that so the caller can
    // fetch again rather than leave the wrong value in place.
    Ok(state
        .apply_full_refresh(&refreshed, &scene_cfgs, &audio_cfgs, generation_before)
        .await)
}

/// Every scene OBS knows, plus the one currently on program.
///
/// Unlike the profile and scene-collection listings below, a failure here
/// fails the whole refresh: a snapshot without a scene list is not worth
/// publishing.
async fn fetch_scene_listing(client: &ObsClient) -> Result<Listing> {
    let scene_list_resp = client.request(requests::get_scene_list()).await?;
    let scenes = extract_resource_names(&scene_list_resp, "scenes", "sceneName")?;

    let current_resp = client
        .request(requests::get_current_program_scene())
        .await?;
    let current_scene = current_resp
        .get("currentProgramSceneName")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            warn!("Malformed GetCurrentProgramScene response: missing `currentProgramSceneName`");
            ObsctlError::ObsRequestFailed(
                "GetCurrentProgramScene response missing `currentProgramSceneName`".to_string(),
            )
        })?;
    let current_scene =
        trim_and_validate_token_with_max_len(current_scene, MAX_TARGET_TOKEN_LENGTH).map_err(
            |error| ObsctlError::ObsRequestFailed(format!("currentProgramSceneName {error}")),
        )?;

    if !scenes.contains(&current_scene) {
        warn!("Current program scene '{current_scene}' is not present in GetSceneList response");
    }

    Ok(Listing::new(scenes, current_scene))
}

/// The mute and volume of every audio input.
async fn fetch_input_states(client: &ObsClient) -> Result<Vec<RawInputState>> {
    let input_names = extract_resource_names(
        &client.request(requests::get_input_list()).await?,
        "inputs",
        "inputName",
    )?;

    // Each input needs two more round-trips. Issued serially this is the
    // dominant cost of a refresh — with ten inputs it was twenty sequential
    // waits — and every millisecond of it is time in which the snapshot being
    // assembled goes further out of date. `join_all` overlaps them instead.
    futures_util::future::join_all(
        input_names
            .iter()
            .map(|name| fetch_input_state(client, name)),
    )
    .await
    .into_iter()
    .collect()
}

/// Which obs-websocket request supplies a "list plus current selection", and
/// under which JSON keys its answer carries the two.
///
/// Profiles and scene collections are read exactly alike and differ only in
/// those names, so the difference lives in this table rather than in two
/// copies of [`fetch_listing`].
struct ListingRequest {
    build: fn() -> crate::obs::protocol::RequestData,
    request_name: &'static str,
    /// Key holding the array of names, e.g. `"profiles"`.
    list_key: &'static str,
    /// Key holding the currently selected name, e.g. `"currentProfileName"`.
    current_key: &'static str,
}

impl ListingRequest {
    const PROFILES: Self = Self {
        build: requests::get_profile_list,
        request_name: "GetProfileList",
        list_key: "profiles",
        current_key: "currentProfileName",
    };

    const SCENE_COLLECTIONS: Self = Self {
        build: requests::get_scene_collection_list,
        request_name: "GetSceneCollectionList",
        list_key: "sceneCollections",
        current_key: "currentSceneCollectionName",
    };
}

/// Read one "list plus current selection" from OBS.
///
/// A failed or malformed reply yields an empty listing rather than an error:
/// profiles and scene collections are secondary information, and losing them
/// should not cost the user the scene and audio state in the same snapshot.
async fn fetch_listing(client: &ObsClient, spec: ListingRequest) -> Listing {
    let Ok(response) = client.request((spec.build)()).await else {
        warn!("Missing {} response", spec.request_name);
        return Listing::new(Vec::new(), String::new());
    };

    let names = crate::obs::validation::extract_string_array(&response, spec.list_key)
        .unwrap_or_else(|_| {
            warn!(
                "Malformed {} response: missing `{}`",
                spec.request_name, spec.list_key
            );
            Vec::new()
        });
    let current = response
        .get(spec.current_key)
        .and_then(|s| s.as_str())
        .unwrap_or_else(|| {
            warn!(
                "Malformed {} response: missing `{}`",
                spec.request_name, spec.current_key
            );
            ""
        })
        .to_string();

    Listing::new(names, current)
}

/// Whether a stream or recording output is running.
///
/// Like [`fetch_listing`], an unreadable reply degrades rather than fails —
/// here to `false`, matching what the panel shows before OBS has answered.
async fn fetch_output_active(
    client: &ObsClient,
    request: crate::obs::protocol::RequestData,
    request_name: &str,
) -> bool {
    client
        .request(request)
        .await
        .ok()
        .and_then(|v| v.get("outputActive").and_then(|b| b.as_bool()))
        .unwrap_or_else(|| {
            warn!("Malformed {request_name} response: missing `outputActive`, defaulting to false");
            false
        })
}

/// Read one input's mute and volume.
///
/// A malformed or failed reply for a single field leaves that field `None`
/// rather than failing the whole refresh: one unreadable input should not cost
/// the user their scene list. Only a request that cannot be built at all — an
/// input name that fails validation — is an error.
async fn fetch_input_state(client: &ObsClient, name: &str) -> Result<RawInputState> {
    let mute_request = requests::get_input_mute(name)?;
    let volume_request = requests::get_input_volume(name)?;

    let muted = client.request(mute_request).await.ok().and_then(|v| {
        match v.get("inputMuted").and_then(|b| b.as_bool()) {
            Some(muted) => Some(muted),
            None => {
                warn!("Malformed GetInputMute response for {name}: missing `inputMuted`");
                None
            }
        }
    });

    let volume_mul = client
        .request(volume_request)
        .await
        .ok()
        .and_then(
            |v| match v.get("inputVolumeMul").and_then(|f| f.as_f64()) {
                Some(volume_mul) if volume_mul.is_finite() && volume_mul >= 0.0 => Some(volume_mul),
                None => {
                    warn!("Malformed GetInputVolume response for {name}: missing `inputVolumeMul`");
                    None
                }
                Some(_) => {
                    warn!(
                        "Malformed GetInputVolume response for {name}: non-finite or negative `inputVolumeMul`"
                    );
                    None
                }
            },
        );

    Ok(RawInputState {
        name: name.to_string(),
        muted,
        volume_mul,
    })
}

const STATS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Poll `GetStats`/`GetStreamStatus`/`GetRecordStatus` on a fixed interval
/// and publish the results via `StateStore::update_stats`. Stream bitrate
/// isn't available directly from obs-websocket, so it's derived from the
/// delta of `GetStreamStatus.outputBytes` between polls (the same approach
/// OBS's own stats dock and most third-party remotes use).
fn spawn_stats_poller(client: ObsClient, state: StateStore) {
    tokio::spawn(async move {
        let mut last_bytes: Option<(u64, tokio::time::Instant)> = None;
        let mut interval = tokio::time::interval(STATS_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            let Ok(stats_resp) = client.request(requests::get_stats()).await else {
                // Client is most likely disconnected; the supervisor's main
                // loop will notice and drop this poller along with it.
                break;
            };
            let stats = ObsStats::from_response(&stats_resp);

            let stream_resp = client.request(requests::get_stream_status()).await.ok();
            let bitrate_kbps = stream_resp.as_ref().and_then(|v| {
                let active = v
                    .get("outputActive")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                let bytes = v.get("outputBytes").and_then(|b| b.as_u64())?;
                if !active {
                    last_bytes = None;
                    return None;
                }
                let now = tokio::time::Instant::now();
                let kbps = last_bytes.and_then(|(prev_bytes, prev_time)| {
                    if bytes < prev_bytes {
                        return None; // stream (re)started; skip this sample
                    }
                    let elapsed = now.duration_since(prev_time).as_secs_f64();
                    (elapsed > 0.0).then(|| (bytes - prev_bytes) as f64 * 8.0 / 1000.0 / elapsed)
                });
                last_bytes = Some((bytes, now));
                kbps
            });
            let stream_duration_ms = stream_resp.as_ref().and_then(output_duration_if_active);

            let record_resp = client.request(requests::get_record_status()).await.ok();
            let record_duration_ms = record_resp.as_ref().and_then(output_duration_if_active);

            state
                .update_stats(stats, bitrate_kbps, stream_duration_ms, record_duration_ms)
                .await;
        }
    });
}

/// Extract `outputDuration` from a `GetStreamStatus`/`GetRecordStatus`
/// response, but only while `outputActive` is true (OBS keeps the last
/// duration around after stopping, which would otherwise look live).
fn output_duration_if_active(response: &serde_json::Value) -> Option<u64> {
    let active = response
        .get("outputActive")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    if !active {
        return None;
    }
    response.get("outputDuration").and_then(|d| d.as_u64())
}

fn log_obs_event(hub: &BroadcastHub, event: &ObsEvent) {
    use crate::obs::client::ObsEvent::*;
    let msg: Option<String> = match event {
        CurrentProgramSceneChanged { scene_name } => {
            Some(format!("OBS: scene changed → {scene_name}"))
        }
        SceneListChanged => Some("OBS: scene list changed".to_string()),
        InputCreated { input_name } => Some(format!("OBS: input created: {input_name}")),
        InputRemoved { input_name } => Some(format!("OBS: input removed: {input_name}")),
        InputMuteStateChanged { input_name, muted } => {
            let state = if *muted { "muted" } else { "unmuted" };
            Some(format!("OBS: {input_name} {state}"))
        }
        InputVolumeChanged {
            input_name,
            volume_db,
            ..
        } => {
            let db = if volume_db.is_finite() {
                format!("{volume_db:.1} dB")
            } else {
                "-∞ dB".to_string()
            };
            Some(format!("OBS: volume changed: {input_name} → {db}"))
        }
        StreamStateChanged { active } => {
            let state = if *active { "started" } else { "stopped" };
            Some(format!("OBS: streaming {state}"))
        }
        RecordStateChanged { active } => {
            let state = if *active { "started" } else { "stopped" };
            Some(format!("OBS: recording {state}"))
        }
        CurrentProfileChanged { profile_name } => {
            Some(format!("OBS: profile changed → {profile_name}"))
        }
        ProfileListChanged => Some("OBS: profile list changed".to_string()),
        CurrentSceneCollectionChanged {
            scene_collection_name,
        } => Some(format!(
            "OBS: scene collection changed → {scene_collection_name}"
        )),
        SceneCollectionListChanged => Some("OBS: scene collection list changed".to_string()),
        // High-frequency or uninteresting — don't flood the log.
        InputVolumeMeters { .. } | Other { .. } => None,
    };
    if let Some(m) = msg {
        hub.publish_log(
            LogEvent::new(LogLevel::Info, m).with_target("obsctl_rs::server::obs_supervisor"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_duration_if_active_returns_none_when_inactive() {
        let v = serde_json::json!({ "outputActive": false, "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), None);
    }

    #[test]
    fn output_duration_if_active_returns_duration_when_active() {
        let v = serde_json::json!({ "outputActive": true, "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), Some(5000));
    }

    #[test]
    fn output_duration_if_active_defaults_missing_active_to_false() {
        let v = serde_json::json!({ "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), None);
    }
}
