use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, watch};
use tracing::{info, warn};

use crate::config::model::Config;
use crate::domain::events::normalize_obs_event;
use crate::ipc::{
    protocol::{LogEvent, LogLevel},
    session::BroadcastHub,
};
use crate::obs::{
    client::{ObsClient, ObsEvent},
    connection::{ObsConnectionParams, connect},
    requests,
};
use crate::runtime::reconnect_policy::ReconnectPolicy;
use crate::server::state_store::{StateStore, build_snapshot};

pub struct ObsSupervisor {
    config: Arc<Mutex<Config>>,
    state: StateStore,
    obs_handle: Arc<Mutex<Option<ObsClient>>>,
    reconnect_rx: mpsc::Receiver<()>,
    shutdown: watch::Receiver<bool>,
    hub: Arc<BroadcastHub>,
}

impl ObsSupervisor {
    pub fn new(
        config: Arc<Mutex<Config>>,
        state: StateStore,
        obs_handle: Arc<Mutex<Option<ObsClient>>>,
        reconnect_rx: mpsc::Receiver<()>,
        shutdown: watch::Receiver<bool>,
        hub: Arc<BroadcastHub>,
    ) -> Self {
        Self {
            config,
            state,
            obs_handle,
            reconnect_rx,
            shutdown,
            hub,
        }
    }

    pub async fn run(mut self) {
        loop {
            let params = {
                let cfg = self.config.lock().await;
                ObsConnectionParams::from_config(&cfg.connection)
            };
            let reconnect_cfg = {
                let cfg = self.config.lock().await;
                cfg.reconnect.clone()
            };
            let mut policy = ReconnectPolicy::new(reconnect_cfg);

            match self.attempt_connect(&params).await {
                Ok((client, obs_version, ws_version)) => {
                    info!("OBS connected: studio={obs_version} ws={ws_version}");
                    self.publish_log(
                        LogLevel::Info,
                        format!("OBS connected: studio={obs_version} ws={ws_version}"),
                    );
                    policy.reset();

                    let fetch_result = self
                        .fetch_and_publish_snapshot(&client, &obs_version, &ws_version)
                        .await;
                    if fetch_result.is_err() {
                        warn!("Initial snapshot fetch failed");
                        self.publish_log(LogLevel::Warn, "Initial OBS snapshot fetch failed");
                    }

                    *self.obs_handle.lock().await = Some(client);

                    // Wait until OBS disconnects or reconnect/shutdown is requested
                    let reason = self.wait_for_disconnect().await;
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
                self.publish_log(
                    LogLevel::Info,
                    "OBS reconnect disabled; supervisor stopping",
                );
                break;
            }
            loop {
                let Some(delay) = policy.next_delay() else {
                    info!("Reconnect exhausted; supervisor stopping");
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
    ) -> crate::domain::result::Result<(ObsClient, String, String)> {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<ObsEvent>(64);
        let state_clone = self.state.clone();
        let hub = Arc::clone(&self.hub);

        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                state_clone.apply_event(event.clone()).await;
                if let Some(payload) = normalize_obs_event(&event) {
                    hub.publish_obs_event(payload);
                }
            }
        });

        connect(params, event_tx).await
    }

    async fn fetch_and_publish_snapshot(
        &self,
        client: &ObsClient,
        obs_version: &str,
        ws_version: &str,
    ) -> crate::domain::result::Result<()> {
        let scene_list_resp = client.request(requests::get_scene_list()).await?;
        let scenes_raw = scene_list_resp
            .get("scenes")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new()));

        let current_resp = client
            .request(requests::get_current_program_scene())
            .await?;
        let current_scene = current_resp
            .get("currentProgramSceneName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let input_list_resp = client.request(requests::get_input_list()).await?;
        let input_names: Vec<String> = input_list_resp
            .get("inputs")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.get("inputName").and_then(|n| n.as_str()))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let mut inputs: Vec<(String, Option<bool>, Option<f64>)> = Vec::new();
        for name in &input_names {
            let muted = client
                .request(requests::get_input_mute(name))
                .await
                .ok()
                .and_then(|v| v.get("inputMuted").and_then(|b| b.as_bool()));

            let vol = client
                .request(requests::get_input_volume(name))
                .await
                .ok()
                .and_then(|v| v.get("inputVolumeMul").and_then(|f| f.as_f64()));

            inputs.push((name.clone(), muted, vol));
        }

        let cfg = self.config.lock().await;
        let snapshot = build_snapshot(
            obs_version,
            ws_version,
            &scenes_raw,
            &current_scene,
            &inputs,
            &cfg.scenes,
            &cfg.audio.inputs,
        );
        drop(cfg);

        self.state.replace(snapshot).await;
        Ok(())
    }

    async fn wait_for_disconnect(&mut self) -> DisconnectReason {
        // We detect OBS disconnect indirectly: the client task drops its channel
        // when the WebSocket closes. We poll for shutdown or explicit reconnect
        // requests here. A more complete impl would use a health check ticker.
        loop {
            tokio::select! {
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
}
