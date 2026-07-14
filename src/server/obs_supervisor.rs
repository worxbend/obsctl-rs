use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, mpsc, watch};
use tracing::{info, warn};

use crate::config::model::Config;
use crate::domain::errors::ObsctlError;
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
use crate::server::state_store::{StateStore, build_snapshot};
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

pub struct ObsSupervisor {
    config: Arc<Mutex<Config>>,
    state: StateStore,
    obs_handle: Arc<Mutex<Option<ObsClient>>>,
    reconnecting: Arc<AtomicBool>,
    reconnect_rx: mpsc::Receiver<()>,
    shutdown: watch::Receiver<bool>,
    hub: Arc<BroadcastHub>,
}

impl ObsSupervisor {
    pub fn new(
        config: Arc<Mutex<Config>>,
        state: StateStore,
        obs_handle: Arc<Mutex<Option<ObsClient>>>,
        reconnecting: Arc<AtomicBool>,
        reconnect_rx: mpsc::Receiver<()>,
        shutdown: watch::Receiver<bool>,
        hub: Arc<BroadcastHub>,
    ) -> Self {
        Self {
            config,
            state,
            obs_handle,
            reconnecting,
            reconnect_rx,
            shutdown,
            hub,
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
    ) -> crate::domain::result::Result<(
        ObsClient,
        String,
        String,
        tokio::sync::oneshot::Receiver<()>,
    )> {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ObsEvent>(64);
        let (client, obs_version, ws_version, disconnect_rx) = connect(params, event_tx).await?;
        let event_client = client.clone();
        let state = self.state.clone();
        let config = Arc::clone(&self.config);
        let hub = Arc::clone(&self.hub);
        let event_obs_version = obs_version.clone();
        let event_ws_version = ws_version.clone();

        spawn_stats_poller(client.clone(), self.state.clone());

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

                if needs_full_refresh
                    && let Err(e) = fetch_and_publish_snapshot_from(
                        &config,
                        &state,
                        &event_client,
                        &event_obs_version,
                        &event_ws_version,
                    )
                    .await
                {
                    warn!("OBS snapshot refresh after scene-list change failed: {e}");
                    hub.publish_log(
                        LogEvent::new(
                            LogLevel::Warn,
                            format!("OBS snapshot refresh after scene-list change failed: {e}"),
                        )
                        .with_target("obsctl_rs::server::obs_supervisor"),
                    );
                }
            }
        });

        Ok((client, obs_version, ws_version, disconnect_rx))
    }

    async fn fetch_and_publish_snapshot(
        &self,
        client: &ObsClient,
        obs_version: &str,
        ws_version: &str,
    ) -> crate::domain::result::Result<()> {
        fetch_and_publish_snapshot_from(&self.config, &self.state, client, obs_version, ws_version)
            .await
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
) -> crate::domain::result::Result<()> {
    let scene_list_resp = client.request(requests::get_scene_list()).await?;
    let scenes_raw = extract_resource_names(&scene_list_resp, "scenes", "sceneName")?;

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

    if !scenes_raw.contains(&current_scene) {
        warn!(
            "Current program scene '{}' is not present in GetSceneList response",
            current_scene
        );
    }

    let input_names = extract_resource_names(
        &client.request(requests::get_input_list()).await?,
        "inputs",
        "inputName",
    )?;

    let mut inputs: Vec<(String, Option<bool>, Option<f64>)> = Vec::new();
    for name in &input_names {
        let muted = client
            .request(requests::get_input_mute(name)?)
            .await
            .ok()
            .and_then(|v| match v.get("inputMuted").and_then(|b| b.as_bool()) {
                Some(muted) => Some(muted),
                None => {
                    warn!("Malformed GetInputMute response for {name}: missing `inputMuted`");
                    None
                }
            });

        let vol = client
            .request(requests::get_input_volume(name)?)
            .await
            .ok()
            .and_then(
                |v| match v.get("inputVolumeMul").and_then(|f| f.as_f64()) {
                    Some(volume_mul) if volume_mul.is_finite() && volume_mul >= 0.0 => Some(volume_mul),
                    None => {
                        warn!(
                            "Malformed GetInputVolume response for {name}: missing `inputVolumeMul`"
                        );
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

        inputs.push((name.clone(), muted, vol));
    }

    let streaming = client
        .request(requests::get_stream_status())
        .await
        .ok()
        .and_then(|v| v.get("outputActive").and_then(|b| b.as_bool()))
        .unwrap_or_else(|| {
            warn!(
                "Malformed GetStreamStatus response: missing `outputActive`, defaulting to false"
            );
            false
        });

    let recording = client
        .request(requests::get_record_status())
        .await
        .ok()
        .and_then(|v| v.get("outputActive").and_then(|b| b.as_bool()))
        .unwrap_or_else(|| {
            warn!(
                "Malformed GetRecordStatus response: missing `outputActive`, defaulting to false"
            );
            false
        });

    let (profiles, current_profile) = client
        .request(requests::get_profile_list())
        .await
        .ok()
        .map(|v| {
            let profiles = crate::obs::validation::extract_string_array(&v, "profiles")
                .ok()
                .unwrap_or_default();
            let current = v
                .get("currentProfileName")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string();
            (profiles, current)
        })
        .unwrap_or_else(|| {
            warn!("Malformed or missing GetProfileList response");
            (Vec::new(), String::new())
        });

    let (scene_collections, current_scene_collection) = client
        .request(requests::get_scene_collection_list())
        .await
        .ok()
        .map(|v| {
            let collections = crate::obs::validation::extract_string_array(&v, "sceneCollections")
                .ok()
                .unwrap_or_default();
            let current = v
                .get("currentSceneCollectionName")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_string();
            (collections, current)
        })
        .unwrap_or_else(|| {
            warn!("Malformed or missing GetSceneCollectionList response");
            (Vec::new(), String::new())
        });

    let cfg = config.lock().await;
    let mut snapshot = build_snapshot(
        obs_version,
        ws_version,
        &scenes_raw,
        &current_scene,
        &inputs,
        &cfg.scenes,
        &cfg.audio.inputs,
        streaming,
        recording,
        &profiles,
        &current_profile,
        &scene_collections,
        &current_scene_collection,
    );
    drop(cfg);

    // build_snapshot always starts stats/bitrate/durations at their zero
    // value; carry over whatever the independent stats poller last saw so a
    // scene/profile-list-triggered full refresh doesn't flicker the live
    // bar back to "waiting" for up to a poll interval.
    let previous = state.read().await;
    snapshot.stats = previous.stats;
    snapshot.stream_bitrate_kbps = previous.stream_bitrate_kbps;
    snapshot.stream_duration_ms = previous.stream_duration_ms;
    snapshot.record_duration_ms = previous.record_duration_ms;

    state.replace(snapshot).await;
    Ok(())
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
