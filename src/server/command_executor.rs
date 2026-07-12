use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::config::{dump as dump_config_mod, model::Config, schema::ValidationWarning};
use crate::domain::{
    aliases::{AliasEntry, resolve, resolve_audio},
    errors::ObsctlError,
    result::Result,
    volume::percent_to_mul,
};
use crate::ipc::{
    protocol::{ErrorPayload, LogEvent, LogLevel, ServerMessage, public_error_code},
    session::{BroadcastHub, CommandDispatch},
};
use crate::obs::{
    client::ObsClient,
    requests,
    state::{ObsSnapshot, ServerStatus},
    validation::extract_resource_names,
};
use crate::server::{client_registry::ClientRegistry, state_store::StateStore};
use crate::support::validation::{
    MAX_TARGET_TOKEN_LENGTH, parse_u8_in_range, trim_and_validate_token_with_max_len,
};

pub struct CommandExecutorConfig {
    pub state: StateStore,
    pub obs: Arc<Mutex<Option<ObsClient>>>,
    pub config: Arc<Mutex<Config>>,
    pub config_path: Option<PathBuf>,
    pub socket_path: PathBuf,
    pub registry: ClientRegistry,
    pub reconnecting: Arc<AtomicBool>,
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
    reconnecting: Arc<AtomicBool>,
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
            reconnecting: cfg.reconnecting,
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
        let result = async {
            let command = parse_server_command(&payload.name)?;
            command.validate_payload(&payload.args)?;

            match command {
                ServerCommand::Ping => Ok(json!({ "message": "pong" })),
                ServerCommand::GetServerStatus => self.cmd_server_status().await,
                ServerCommand::GetObsStatus => self.cmd_obs_status().await,
                ServerCommand::GetSnapshot => self.cmd_get_snapshot().await,
                ServerCommand::SetScene => self.cmd_set_scene(&payload.args).await,
                ServerCommand::SetProfile => self.cmd_set_profile(&payload.args).await,
                ServerCommand::Mute => self.cmd_set_mute(&payload.args, true).await,
                ServerCommand::Unmute => self.cmd_set_mute(&payload.args, false).await,
                ServerCommand::ToggleMute => self.cmd_toggle_mute(&payload.args).await,
                ServerCommand::SetVolume => self.cmd_set_volume(&payload.args).await,
                ServerCommand::ValidateConfig => self.cmd_validate_config().await,
                ServerCommand::ReloadConfig => self.cmd_reload_config().await,
                ServerCommand::ReconnectObs => self.cmd_reconnect_obs().await,
                ServerCommand::ShutdownServer => self.cmd_shutdown_server().await,
                ServerCommand::DumpConfig => self.cmd_dump_config().await,
                ServerCommand::ToggleStream => self.cmd_toggle_stream().await,
                ServerCommand::ToggleRecord => self.cmd_toggle_record().await,
            }
        }
        .await;

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
            reconnecting: self.reconnecting.load(Ordering::Relaxed),
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
            .request(requests::set_current_program_scene(&obs_name)?)
            .await?;
        info!("Scene set to: {obs_name}");
        Ok(json!({ "message": format!("scene set: {obs_name}") }))
    }

    async fn cmd_set_profile(&self, args: &Value) -> crate::domain::result::Result<Value> {
        let target = required_string(args, "target")?;
        let client = self.require_obs().await?;

        let snap = self.state.read().await;
        let known = snap.profiles.iter().any(|p| p == &target);
        drop(snap);
        if !known {
            return Err(ObsctlError::ObsRequestFailed(format!(
                "unknown profile: {target}"
            )));
        }

        client
            .request(requests::set_current_profile(&target)?)
            .await?;
        info!("Profile set to: {target}");
        Ok(json!({ "message": format!("profile set: {target}") }))
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
            .request(requests::set_input_mute(&obs_name, muted)?)
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
            .request(requests::toggle_input_mute(&obs_name)?)
            .await?;
        Ok(json!({ "message": format!("mute toggled: {obs_name}") }))
    }

    async fn cmd_set_volume(&self, args: &Value) -> crate::domain::result::Result<Value> {
        let target = required_string(args, "target")?;
        let percent = required_u8_percentage(args, "percent")?;

        let client = self.require_obs().await?;
        let snap = self.state.read().await;

        let entries = audio_alias_entries(&snap);
        drop(snap);

        let resolved = resolve_audio(&target, &entries)?;
        let obs_name = resolved.name.clone();
        let vol_mul = percent_to_mul(percent);

        client
            .request(requests::set_input_volume(&obs_name, vol_mul)?)
            .await?;
        Ok(json!({ "message": format!("volume set to {percent}%: {obs_name}") }))
    }

    async fn cmd_toggle_stream(&self) -> crate::domain::result::Result<Value> {
        let client = self.require_obs().await?;
        let result = client.request(requests::toggle_stream()).await?;
        let active = match result.get("outputActive").and_then(|v| v.as_bool()) {
            Some(active) => Some(active),
            None => {
                warn!("Malformed toggle_stream response: missing or invalid outputActive");
                None
            }
        };
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
        let active = match result.get("outputActive").and_then(|v| v.as_bool()) {
            Some(active) => Some(active),
            None => {
                warn!("Malformed toggle_record response: missing or invalid outputActive");
                None
            }
        };
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
        let scenes = extract_resource_names(&scene_resp, "scenes", "sceneName")?;

        let input_resp = client
            .request(crate::obs::requests::get_input_list())
            .await?;
        let inputs = extract_resource_names(&input_resp, "inputs", "inputName")?;

        let obs_resources = dump_config_mod::ObsResources { scenes, inputs };

        let config_guard = self.config.lock().await;
        let current_config = config_guard.clone();
        drop(config_guard);

        let merged = dump_config_mod::merge(&current_config, &obs_resources)?;
        let mut reload_warnings = Vec::new();
        let mut reload_failed = false;
        let mut reload_error = None;

        if let Some(path) = &self.config_path {
            let backup = dump_config_mod::write_backup(path)?;
            crate::config::writer::write_atomic(&merged, path)?;
            info!(
                "Config dumped to {} (backup: {})",
                path.display(),
                backup.display()
            );

            // Reload config in memory.
            match crate::config::loader::load_with_warnings(path) {
                Ok((new_cfg, warnings)) => {
                    self.log_config_warnings(&warnings, "after dump reload");
                    reload_warnings = Self::warning_messages(&warnings);

                    let mut guard = self.config.lock().await;
                    *guard = new_cfg;
                }
                Err(e) => {
                    warn!("Failed to reload config after dump: {e}");
                    self.publish_log(
                        LogLevel::Warn,
                        format!("Config reload after dump failed: {e}"),
                    );
                    reload_error = Some(e.to_string());
                    reload_failed = true;
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
            "reload_failed": reload_failed,
            "warnings": reload_warnings,
            "reload_error": reload_error,
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
            Ok(warnings) => {
                self.publish_log(LogLevel::Info, "Config reloaded");
                let warning_count = warnings.len();
                if warning_count > 0 {
                    self.publish_log(
                        LogLevel::Info,
                        format!("Config reloaded with {warning_count} warning(s)"),
                    );
                }
            }
            Err(e) => {
                warn!("Config reload failed: {e}");
                self.publish_log(LogLevel::Warn, format!("Config reload failed: {e}"));
            }
        }

        let warnings = result?;
        let warning_msgs = Self::warning_messages(&warnings);
        Ok(json!({
            "message": "config reloaded",
            "warnings": warning_msgs,
        }))
    }

    async fn reload_config_from_disk(
        &self,
    ) -> crate::domain::result::Result<Vec<ValidationWarning>> {
        let path = self.config_path.as_ref().ok_or_else(|| {
            ObsctlError::ConfigInvalid("no config path configured for reload".to_string())
        })?;

        let (new_config, warnings) = crate::config::loader::load_with_warnings(path)?;
        self.log_config_warnings(&warnings, "on reload");

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
        Ok(warnings)
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

    fn log_config_warnings(&self, warnings: &[ValidationWarning], context: &str) {
        for warning in warnings {
            let message = format!("Config warning {context}: {}", warning.0);
            warn!("{message}");
            self.publish_log(LogLevel::Warn, message);
        }
    }

    fn warning_messages(warnings: &[ValidationWarning]) -> Vec<String> {
        warnings.iter().map(|warning| warning.0.clone()).collect()
    }

    async fn require_obs(&self) -> crate::domain::result::Result<ObsClient> {
        self.obs
            .lock()
            .await
            .clone()
            .ok_or(ObsctlError::ObsUnavailable)
    }
}

#[cfg(test)]
fn validate_command_payload(command: &str, args: &Value) -> Result<()> {
    let command = parse_server_command(command)?;
    command.validate_payload(args)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServerCommand {
    Ping,
    GetServerStatus,
    GetObsStatus,
    GetSnapshot,
    SetScene,
    SetProfile,
    Mute,
    Unmute,
    ToggleMute,
    SetVolume,
    ValidateConfig,
    ReloadConfig,
    ReconnectObs,
    ShutdownServer,
    DumpConfig,
    ToggleStream,
    ToggleRecord,
}

impl ServerCommand {
    fn validate_payload(&self, args: &Value) -> Result<()> {
        match self {
            Self::Ping
            | Self::GetServerStatus
            | Self::GetObsStatus
            | Self::GetSnapshot
            | Self::ValidateConfig
            | Self::ReloadConfig
            | Self::ReconnectObs
            | Self::ShutdownServer
            | Self::DumpConfig
            | Self::ToggleStream
            | Self::ToggleRecord => validate_empty_payload(args, self.name()),
            Self::SetScene | Self::SetProfile => {
                validate_object_args(args, self.name(), &["target"])
            }
            Self::Mute | Self::Unmute | Self::ToggleMute => {
                validate_object_args(args, self.name(), &["target"])
            }
            Self::SetVolume => validate_object_args(args, self.name(), &["target", "percent"]),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::GetServerStatus => "get_server_status",
            Self::GetObsStatus => "get_obs_status",
            Self::GetSnapshot => "get_snapshot",
            Self::SetScene => "set_scene",
            Self::SetProfile => "set_profile",
            Self::Mute => "mute",
            Self::Unmute => "unmute",
            Self::ToggleMute => "toggle_mute",
            Self::SetVolume => "set_volume",
            Self::ValidateConfig => "validate_config",
            Self::ReloadConfig => "reload_config",
            Self::ReconnectObs => "reconnect_obs",
            Self::ShutdownServer => "shutdown_server",
            Self::DumpConfig => "dump_config",
            Self::ToggleStream => "toggle_stream",
            Self::ToggleRecord => "toggle_record",
        }
    }
}

fn parse_server_command(name: &str) -> Result<ServerCommand> {
    match name {
        "ping" => Ok(ServerCommand::Ping),
        "get_server_status" => Ok(ServerCommand::GetServerStatus),
        "get_obs_status" => Ok(ServerCommand::GetObsStatus),
        "get_snapshot" => Ok(ServerCommand::GetSnapshot),
        "set_scene" => Ok(ServerCommand::SetScene),
        "set_profile" => Ok(ServerCommand::SetProfile),
        "mute" => Ok(ServerCommand::Mute),
        "unmute" => Ok(ServerCommand::Unmute),
        "toggle_mute" => Ok(ServerCommand::ToggleMute),
        "set_volume" => Ok(ServerCommand::SetVolume),
        "validate_config" => Ok(ServerCommand::ValidateConfig),
        "reload_config" => Ok(ServerCommand::ReloadConfig),
        "reconnect_obs" => Ok(ServerCommand::ReconnectObs),
        "shutdown_server" => Ok(ServerCommand::ShutdownServer),
        "dump_config" => Ok(ServerCommand::DumpConfig),
        "toggle_stream" => Ok(ServerCommand::ToggleStream),
        "toggle_record" => Ok(ServerCommand::ToggleRecord),
        _ => Err(ObsctlError::CommandParseError(format!(
            "unknown command: {name}"
        ))),
    }
}

fn required_string(args: &Value, key: &str) -> crate::domain::result::Result<String> {
    let raw = args
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))?;

    trim_and_validate_token_with_max_len(&raw, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::CommandParseError(format!("{key} {error}")))
}

fn required_u8_percentage(args: &Value, key: &str) -> crate::domain::result::Result<u8> {
    let value = args
        .get(key)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))?;

    let percent = if let Some(percent) = value.as_u64() {
        parse_u8_in_range(&percent.to_string(), key, 0, 100)
            .map_err(|error| ObsctlError::CommandParseError(error.to_string()))?
    } else {
        return Err(ObsctlError::CommandParseError(format!(
            "{key} must be an integer 0-100"
        )));
    };

    Ok(percent)
}

fn validate_object_args(args: &Value, command: &str, required: &[&str]) -> Result<()> {
    let object = args.as_object().ok_or_else(|| {
        ObsctlError::CommandParseError(format!("command {command} requires an object payload"))
    })?;

    for (key, _) in object {
        if !required.contains(&key.as_str()) {
            return Err(ObsctlError::CommandParseError(format!(
                "command {command} received unexpected argument '{key}'"
            )));
        }
    }

    for key in required {
        if !object.contains_key(*key) {
            return Err(ObsctlError::CommandParseError(format!(
                "command {command} missing required argument '{key}'"
            )));
        }
    }

    Ok(())
}

fn validate_empty_payload(args: &Value, command: &str) -> Result<()> {
    if args.is_null() {
        return Ok(());
    }

    if let Some(object) = args.as_object()
        && object.is_empty()
    {
        return Ok(());
    }

    Err(ObsctlError::CommandParseError(format!(
        "command {command} does not accept arguments"
    )))
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

#[cfg(test)]
mod tests {
    use super::ServerCommand;
    use super::{
        MAX_TARGET_TOKEN_LENGTH, parse_server_command, required_string, required_u8_percentage,
    };
    use super::{validate_command_payload, validate_empty_payload, validate_object_args};
    use serde_json::json;

    #[test]
    fn required_string_rejects_control_characters_and_empty_values() {
        let args = json!({ "target": "\t" });
        assert!(required_string(&args, "target").is_err());

        let args = json!({ "target": "" });
        assert!(required_string(&args, "target").is_err());

        let args = json!({ "target": 42 });
        assert!(required_string(&args, "target").is_err());

        let args = json!({ "target": " Main Scene " });
        assert_eq!(
            required_string(&args, "target").unwrap(),
            "Main Scene".to_string()
        );

        let args = json!({ "target": "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1) });
        assert!(required_string(&args, "target").is_err());
    }

    #[test]
    fn required_u8_percentage_requires_integer_in_range() {
        let args = json!({ "percent": 42 });
        assert_eq!(required_u8_percentage(&args, "percent").unwrap(), 42);

        let args = json!({ "percent": 150 });
        assert!(required_u8_percentage(&args, "percent").is_err());

        let args = json!({ "percent": -1 });
        assert!(required_u8_percentage(&args, "percent").is_err());

        let args = json!({ "percent": 50.5 });
        assert!(required_u8_percentage(&args, "percent").is_err());

        let args = json!({});
        assert!(required_u8_percentage(&args, "percent").is_err());
    }

    #[test]
    fn validate_object_args_rejects_extra_payload_fields() {
        let args = json!({
            "target": "Mic",
            "extra": "boom",
        });
        assert!(validate_object_args(&args, "mute", &["target"]).is_err());
    }

    #[test]
    fn validate_object_args_rejects_missing_payload_fields() {
        let args = json!({
            "target": "Mic",
        });
        assert!(validate_object_args(&args, "set_volume", &["target", "percent"]).is_err());
    }

    #[test]
    fn validate_object_args_rejects_non_object_payload() {
        assert!(validate_object_args(&json!(null), "set_scene", &["target"]).is_err());
        assert!(validate_object_args(&json!("string"), "set_scene", &["target"]).is_err());
    }

    #[test]
    fn validate_empty_payload_rejects_argument_objects() {
        assert!(validate_empty_payload(&json!({ "extra": true }), "ping").is_err());
    }

    #[test]
    fn validate_empty_payload_rejects_non_empty_non_object_payload() {
        assert!(validate_empty_payload(&json!([]), "ping").is_err());
        assert!(validate_empty_payload(&json!("x"), "ping").is_err());
    }

    #[test]
    fn validate_empty_payload_allows_empty_object_or_null() {
        assert!(validate_empty_payload(&json!(null), "ping").is_ok());
        assert!(validate_empty_payload(&json!({}), "ping").is_ok());
    }

    #[test]
    fn validate_command_payload_rejects_unknown_command() {
        assert!(validate_command_payload("does-not-exist", &json!(null)).is_err());
    }

    #[test]
    fn validate_command_payload_rejects_wrong_shape_per_command() {
        assert!(validate_command_payload("set_volume", &json!({ "target": "Mic" })).is_err());
        assert!(validate_command_payload("toggle_stream", &json!({ "extra": true }),).is_err());
    }

    #[test]
    fn parse_server_command_rejects_unknown_name() {
        assert!(parse_server_command("does-not-exist").is_err());
    }

    #[test]
    fn parse_server_command_covers_known_commands() {
        let cases = [
            ("ping", ServerCommand::Ping),
            ("get_server_status", ServerCommand::GetServerStatus),
            ("get_obs_status", ServerCommand::GetObsStatus),
            ("get_snapshot", ServerCommand::GetSnapshot),
            ("set_scene", ServerCommand::SetScene),
            ("set_profile", ServerCommand::SetProfile),
            ("mute", ServerCommand::Mute),
            ("unmute", ServerCommand::Unmute),
            ("toggle_mute", ServerCommand::ToggleMute),
            ("set_volume", ServerCommand::SetVolume),
            ("validate_config", ServerCommand::ValidateConfig),
            ("reload_config", ServerCommand::ReloadConfig),
            ("reconnect_obs", ServerCommand::ReconnectObs),
            ("shutdown_server", ServerCommand::ShutdownServer),
            ("dump_config", ServerCommand::DumpConfig),
            ("toggle_stream", ServerCommand::ToggleStream),
            ("toggle_record", ServerCommand::ToggleRecord),
        ];
        for (name, expected) in cases {
            assert_eq!(parse_server_command(name).unwrap(), expected);
        }
    }
}
