use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::config::{dump as dump_config_mod, model::Config};
use crate::domain::{
    aliases::{AliasEntry, resolve, resolve_audio},
    errors::ObsctlError,
    volume::percent_to_mul,
};
use crate::ipc::{
    protocol::{ErrorPayload, LogEvent, LogLevel, ServerMessage, public_error_code},
    session::{BroadcastHub, CommandDispatch},
};
use crate::obs::{client::ObsClient, requests, state::{ObsSnapshot, ServerStatus}};
use crate::server::{client_registry::ClientRegistry, state_store::StateStore};

pub struct CommandExecutorConfig {
    pub state: StateStore,
    pub obs: Arc<Mutex<Option<ObsClient>>>,
    pub config: Arc<Mutex<Config>>,
    pub config_path: Option<PathBuf>,
    pub socket_path: PathBuf,
    pub registry: ClientRegistry,
    pub reconnect_tx: mpsc::Sender<()>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub hub: Arc<BroadcastHub>,
}

pub struct CommandExecutor {
    state: StateStore,
    obs: Arc<Mutex<Option<ObsClient>>>,
    config: Arc<Mutex<Config>>,
    config_path: Option<PathBuf>,
    socket_path: PathBuf,
    registry: ClientRegistry,
    started_at: Instant,
    reconnect_tx: mpsc::Sender<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    hub: Arc<BroadcastHub>,
}

impl CommandExecutor {
    pub fn new(cfg: CommandExecutorConfig) -> Self {
        Self {
            state: cfg.state,
            obs: cfg.obs,
            config: cfg.config,
            config_path: cfg.config_path,
            socket_path: cfg.socket_path,
            registry: cfg.registry,
            started_at: Instant::now(),
            reconnect_tx: cfg.reconnect_tx,
            shutdown_tx: cfg.shutdown_tx,
            hub: cfg.hub,
        }
    }

    pub async fn run(self, mut rx: mpsc::Receiver<CommandDispatch>) {
        while let Some(dispatch) = rx.recv().await {
            let response = self.handle(dispatch.id.clone(), dispatch.payload).await;
            let _ = dispatch.reply.send(response);
        }
    }

    async fn handle(
        &self,
        id: String,
        payload: crate::ipc::protocol::CommandPayload,
    ) -> ServerMessage {
        debug!("Command id={id} name={}", payload.name);
        let result = match payload.name.as_str() {
            "ping" => Ok(json!({ "message": "pong" })),
            "get_server_status" => self.cmd_server_status().await,
            "get_obs_status" => self.cmd_obs_status().await,
            "get_snapshot" => self.cmd_get_snapshot().await,
            "set_scene" => self.cmd_set_scene(&payload.args).await,
            "mute" => self.cmd_set_mute(&payload.args, true).await,
            "unmute" => self.cmd_set_mute(&payload.args, false).await,
            "toggle_mute" => self.cmd_toggle_mute(&payload.args).await,
            "set_volume" => self.cmd_set_volume(&payload.args).await,
            "validate_config" => self.cmd_validate_config().await,
            "reload_config" => self.cmd_reload_config().await,
            "reconnect_obs" => self.cmd_reconnect_obs().await,
            "shutdown_server" => self.cmd_shutdown_server().await,
            "dump_config" => self.cmd_dump_config().await,
            "toggle_stream" => self.cmd_toggle_stream().await,
            "toggle_record" => self.cmd_toggle_record().await,
            other => Err(ObsctlError::CommandParseError(format!(
                "unknown command: {other}"
            ))),
        };

        match result {
            Ok(data) => ServerMessage::Response {
                id,
                ok: true,
                result: Some(data),
                error: None,
            },
            Err(e) => {
                let code = public_error_code(&e);
                ServerMessage::Response {
                    id,
                    ok: false,
                    result: None,
                    error: Some(ErrorPayload::new(code, e.to_string())),
                }
            }
        }
    }

    async fn cmd_server_status(&self) -> crate::domain::result::Result<Value> {
        let snap = self.state.read().await;
        let status = ServerStatus {
            pid: std::process::id(),
            uptime_seconds: self.started_at.elapsed().as_secs(),
            socket_path: self.socket_path.clone(),
            client_count: self.registry.count(),
            obs_connected: snap.connected,
            reconnecting: false, // supervisor sets this separately
            last_error: snap.last_error.clone(),
        };
        drop(snap);
        serde_json::to_value(status).map_err(|e| ObsctlError::ObsRequestFailed(e.to_string()))
    }

    async fn cmd_obs_status(&self) -> crate::domain::result::Result<Value> {
        let snap = self.state.read().await;
        Ok(json!({
            "connected": snap.connected,
            "current_scene": snap.current_scene,
            "obs_studio_version": snap.obs_studio_version,
            "obs_websocket_version": snap.obs_websocket_version,
            "last_error": snap.last_error,
        }))
    }

    async fn cmd_get_snapshot(&self) -> crate::domain::result::Result<Value> {
        let snap = self.state.read().await;
        serde_json::to_value(snap).map_err(|e| ObsctlError::ObsRequestFailed(e.to_string()))
    }

    async fn cmd_set_scene(&self, args: &Value) -> crate::domain::result::Result<Value> {
        let target = required_string(args, "target")?;
        let client = self.require_obs().await?;
        let snap = self.state.read().await;

        let entries = scene_alias_entries(&snap);
        drop(snap);

        let resolved = resolve(&target, &entries)?;
        let obs_name = resolved.name.clone();

        client
            .request(requests::set_current_program_scene(&obs_name))
            .await?;
        info!("Scene set to: {obs_name}");
        Ok(json!({ "message": format!("scene set: {obs_name}") }))
    }

    async fn cmd_set_mute(
        &self,
        args: &Value,
        muted: bool,
    ) -> crate::domain::result::Result<Value> {
        let target = required_string(args, "target")?;
        let client = self.require_obs().await?;
        let snap = self.state.read().await;

        let entries = audio_alias_entries(&snap);
        drop(snap);

        let resolved = resolve_audio(&target, &entries)?;
        let obs_name = resolved.name.clone();

        client
            .request(requests::set_input_mute(&obs_name, muted))
            .await?;
        let action = if muted { "muted" } else { "unmuted" };
        Ok(json!({ "message": format!("{action}: {obs_name}") }))
    }

    async fn cmd_toggle_mute(&self, args: &Value) -> crate::domain::result::Result<Value> {
        let target = required_string(args, "target")?;
        let client = self.require_obs().await?;
        let snap = self.state.read().await;

        let entries = audio_alias_entries(&snap);
        drop(snap);

        let resolved = resolve_audio(&target, &entries)?;
        let obs_name = resolved.name.clone();

        client
            .request(requests::toggle_input_mute(&obs_name))
            .await?;
        Ok(json!({ "message": format!("mute toggled: {obs_name}") }))
    }

    async fn cmd_set_volume(&self, args: &Value) -> crate::domain::result::Result<Value> {
        let target = required_string(args, "target")?;
        let percent = args
            .get("percent")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ObsctlError::CommandParseError("missing percent".to_string()))?;
        if percent > 100 {
            return Err(ObsctlError::CommandParseError(
                "percent must be 0-100".to_string(),
            ));
        }
        let percent = percent as u8;

        let client = self.require_obs().await?;
        let snap = self.state.read().await;

        let entries = audio_alias_entries(&snap);
        drop(snap);

        let resolved = resolve_audio(&target, &entries)?;
        let obs_name = resolved.name.clone();
        let vol_mul = percent_to_mul(percent);

        client
            .request(requests::set_input_volume(&obs_name, vol_mul))
            .await?;
        Ok(json!({ "message": format!("volume set to {percent}%: {obs_name}") }))
    }

    async fn cmd_toggle_stream(&self) -> crate::domain::result::Result<Value> {
        let client = self.require_obs().await?;
        let result = client.request(requests::toggle_stream()).await?;
        let active = result.get("outputActive").and_then(|v| v.as_bool());
        let state = match active {
            Some(true) => "started",
            Some(false) => "stopped",
            None => "toggled",
        };
        info!("Streaming {state}");
        Ok(json!({ "message": format!("streaming {state}") }))
    }

    async fn cmd_toggle_record(&self) -> crate::domain::result::Result<Value> {
        let client = self.require_obs().await?;
        let result = client.request(requests::toggle_record()).await?;
        let active = result.get("outputActive").and_then(|v| v.as_bool());
        let state = match active {
            Some(true) => "started",
            Some(false) => "stopped",
            None => "toggled",
        };
        info!("Recording {state}");
        Ok(json!({ "message": format!("recording {state}") }))
    }

    async fn cmd_dump_config(&self) -> crate::domain::result::Result<Value> {
        let client = self.require_obs().await?;

        // Fetch scene and input lists from OBS.
        let scene_resp = client
            .request(crate::obs::requests::get_scene_list())
            .await?;
        let scenes: Vec<String> = scene_resp
            .get("scenes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.get("sceneName").and_then(|n| n.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let input_resp = client
            .request(crate::obs::requests::get_input_list())
            .await?;
        let inputs: Vec<String> = input_resp
            .get("inputs")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.get("inputName").and_then(|n| n.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let obs_resources = dump_config_mod::ObsResources { scenes, inputs };

        let config_guard = self.config.lock().await;
        let current_config = config_guard.clone();
        drop(config_guard);

        let merged = dump_config_mod::merge(&current_config, &obs_resources)?;

        if let Some(path) = &self.config_path {
            let backup = dump_config_mod::write_backup(path)?;
            dump_config_mod::write_atomic(&merged, path)?;
            info!(
                "Config dumped to {} (backup: {})",
                path.display(),
                backup.display()
            );

            // Reload config in memory.
            match crate::config::loader::load(path) {
                Ok(new_cfg) => {
                    let mut guard = self.config.lock().await;
                    *guard = new_cfg;
                }
                Err(e) => {
                    warn!("Failed to reload config after dump: {e}");
                    self.publish_log(
                        LogLevel::Warn,
                        format!("Config reload after dump failed: {e}"),
                    );
                }
            }
        } else {
            return Err(crate::domain::errors::ObsctlError::ConfigInvalid(
                "dump-config requires a config file path".to_string(),
            ));
        }

        let scene_count = merged.scenes.len();
        let input_count = merged.audio.inputs.len();
        Ok(json!({
            "message": format!("config dumped: {scene_count} scenes, {input_count} inputs"),
            "scenes": scene_count,
            "inputs": input_count,
        }))
    }

    async fn cmd_validate_config(&self) -> crate::domain::result::Result<Value> {
        let config = self.config.lock().await;
        let warnings = crate::config::schema::validate(&config)?;
        let warning_msgs: Vec<String> = warnings.iter().map(|w| w.0.clone()).collect();
        Ok(json!({ "valid": true, "warnings": warning_msgs }))
    }

    async fn cmd_reload_config(&self) -> crate::domain::result::Result<Value> {
        let result = self.reload_config_from_disk().await;
        match &result {
            Ok(()) => {
                self.publish_log(LogLevel::Info, "Config reloaded");
            }
            Err(e) => {
                warn!("Config reload failed: {e}");
                self.publish_log(LogLevel::Warn, format!("Config reload failed: {e}"));
            }
        }

        result?;
        Ok(json!({ "message": "config reloaded" }))
    }

    async fn reload_config_from_disk(&self) -> crate::domain::result::Result<()> {
        let path = self.config_path.as_ref().ok_or_else(|| {
            ObsctlError::ConfigInvalid("no config path configured for reload".to_string())
        })?;

        let new_config = crate::config::loader::load(path)?;
        crate::config::schema::validate(&new_config)?;

        let scenes = new_config.scenes.clone();
        let audio_inputs = new_config.audio.inputs.clone();

        {
            let mut guard = self.config.lock().await;
            *guard = new_config;
        }

        self.state.merge_config(&scenes, &audio_inputs).await;
        // Re-broadcast current snapshot so subscribers see updated alias/shortcut metadata.
        let snapshot = self.state.read().await;
        self.state.replace(snapshot).await;

        info!("Config reloaded from {}", path.display());
        Ok(())
    }

    async fn cmd_reconnect_obs(&self) -> crate::domain::result::Result<Value> {
        let _ = self.reconnect_tx.send(()).await;
        Ok(json!({ "message": "reconnect triggered" }))
    }

    async fn cmd_shutdown_server(&self) -> crate::domain::result::Result<Value> {
        let config = self.config.lock().await;
        if !config.server.allow_remote_shutdown {
            return Err(ObsctlError::ShutdownDisabled);
        }
        drop(config);
        info!("Shutdown requested via IPC");
        self.publish_log(LogLevel::Info, "Shutdown requested via IPC");
        let _ = self.shutdown_tx.send(true);
        Ok(json!({ "message": "shutdown initiated" }))
    }

    fn publish_log(&self, level: LogLevel, message: impl AsRef<str>) {
        self.hub.publish_log(
            LogEvent::new(level, message).with_target("obsctl_rs::server::command_executor"),
        );
    }

    async fn require_obs(&self) -> crate::domain::result::Result<ObsClient> {
        self.obs
            .lock()
            .await
            .clone()
            .ok_or(ObsctlError::ObsUnavailable)
    }
}

fn required_string(args: &Value, key: &str) -> crate::domain::result::Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))
}

fn scene_alias_entries(snap: &ObsSnapshot) -> Vec<AliasEntry> {
    snap.scenes
        .iter()
        .map(|s| AliasEntry {
            name: s.name.clone(),
            alias: s.alias.clone(),
            shortcut: s.shortcut.clone(),
        })
        .collect()
}

fn audio_alias_entries(snap: &ObsSnapshot) -> Vec<AliasEntry> {
    snap.audio_inputs
        .iter()
        .map(|a| AliasEntry {
            name: a.name.clone(),
            alias: a.alias.clone(),
            shortcut: a.shortcut.clone(),
        })
        .collect()
}
