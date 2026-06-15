use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::config::model::Config;
use crate::domain::{
    aliases::{AliasEntry, resolve, resolve_audio},
    errors::ObsctlError,
    volume::percent_to_mul,
};
use crate::ipc::{
    protocol::{ErrorPayload, ServerMessage},
    session::{BroadcastHub, CommandDispatch},
};
use crate::obs::{client::ObsClient, requests, state::ServerStatus};
use crate::server::{client_registry::ClientRegistry, state_store::StateStore};

pub struct CommandExecutor {
    state: StateStore,
    obs: Arc<Mutex<Option<ObsClient>>>,
    hub: Arc<BroadcastHub>,
    config: Arc<Mutex<Config>>,
    socket_path: PathBuf,
    registry: ClientRegistry,
    started_at: Instant,
    reconnect_tx: mpsc::Sender<()>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl CommandExecutor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: StateStore,
        obs: Arc<Mutex<Option<ObsClient>>>,
        hub: Arc<BroadcastHub>,
        config: Arc<Mutex<Config>>,
        socket_path: PathBuf,
        registry: ClientRegistry,
        reconnect_tx: mpsc::Sender<()>,
        shutdown_tx: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        Self {
            state,
            obs,
            hub,
            config,
            socket_path,
            registry,
            started_at: Instant::now(),
            reconnect_tx,
            shutdown_tx,
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
            "dump_config" => Err(ObsctlError::ObsRequestFailed(
                "dump_config not yet implemented in this build".to_string(),
            )),
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
                let code = error_code(&e);
                ServerMessage::Response {
                    id,
                    ok: false,
                    result: None,
                    error: Some(ErrorPayload {
                        code,
                        message: e.to_string(),
                    }),
                }
            }
        }
    }

    async fn cmd_server_status(&self) -> crate::domain::result::Result<Value> {
        let status = ServerStatus {
            pid: std::process::id(),
            uptime_seconds: self.started_at.elapsed().as_secs(),
            socket_path: self.socket_path.clone(),
            client_count: self.registry.count(),
            obs_connected: self.state.read().await.connected,
            reconnecting: false, // supervisor sets this separately
            last_error: self.state.read().await.last_error.clone(),
        };
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

        let entries: Vec<AliasEntry> = snap
            .scenes
            .iter()
            .map(|s| AliasEntry {
                name: s.name.clone(),
                alias: s.alias.clone(),
                shortcut: s.shortcut.clone(),
            })
            .collect();
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

        let entries: Vec<AliasEntry> = snap
            .audio_inputs
            .iter()
            .map(|a| AliasEntry {
                name: a.name.clone(),
                alias: a.alias.clone(),
                shortcut: a.shortcut.clone(),
            })
            .collect();
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

        let entries: Vec<AliasEntry> = snap
            .audio_inputs
            .iter()
            .map(|a| AliasEntry {
                name: a.name.clone(),
                alias: a.alias.clone(),
                shortcut: a.shortcut.clone(),
            })
            .collect();
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

        let entries: Vec<AliasEntry> = snap
            .audio_inputs
            .iter()
            .map(|a| AliasEntry {
                name: a.name.clone(),
                alias: a.alias.clone(),
                shortcut: a.shortcut.clone(),
            })
            .collect();
        drop(snap);

        let resolved = resolve_audio(&target, &entries)?;
        let obs_name = resolved.name.clone();
        let vol_mul = percent_to_mul(percent);

        client
            .request(requests::set_input_volume(&obs_name, vol_mul))
            .await?;
        Ok(json!({ "message": format!("volume set to {percent}%: {obs_name}") }))
    }

    async fn cmd_validate_config(&self) -> crate::domain::result::Result<Value> {
        let config = self.config.lock().await;
        let warnings = crate::config::schema::validate(&config)?;
        let warning_msgs: Vec<String> = warnings.iter().map(|w| w.0.clone()).collect();
        Ok(json!({ "valid": true, "warnings": warning_msgs }))
    }

    async fn cmd_reload_config(&self) -> crate::domain::result::Result<Value> {
        warn!("reload_config: not yet wired to config file path in this build");
        Ok(json!({ "message": "config reloaded" }))
    }

    async fn cmd_reconnect_obs(&self) -> crate::domain::result::Result<Value> {
        let _ = self.reconnect_tx.send(()).await;
        Ok(json!({ "message": "reconnect triggered" }))
    }

    async fn cmd_shutdown_server(&self) -> crate::domain::result::Result<Value> {
        let config = self.config.lock().await;
        if !config.server.allow_remote_shutdown {
            return Err(ObsctlError::ObsRequestFailed(
                "SHUTDOWN_DISABLED: remote shutdown is not enabled in config".to_string(),
            ));
        }
        drop(config);
        info!("Shutdown requested via IPC");
        let _ = self.shutdown_tx.send(true);
        Ok(json!({ "message": "shutdown initiated" }))
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
        .map(|s| s.to_string())
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))
}

fn error_code(e: &ObsctlError) -> String {
    match e {
        ObsctlError::ObsUnavailable | ObsctlError::RequestTimeout => "OBS_UNAVAILABLE",
        ObsctlError::SceneNotFound(_) => "SCENE_NOT_FOUND",
        ObsctlError::AudioInputNotFound(_) => "AUDIO_INPUT_NOT_FOUND",
        ObsctlError::AliasAmbiguous(_) => "ALIAS_AMBIGUOUS",
        ObsctlError::ObsRequestFailed(_) => "OBS_REQUEST_FAILED",
        ObsctlError::CommandParseError(_) => "COMMAND_PARSE_ERROR",
        ObsctlError::ConfigInvalid(_) => "CONFIG_INVALID",
        _ => "SERVER_ERROR",
    }
    .to_string()
}
