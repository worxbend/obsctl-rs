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
    protocol::{ErrorPayload, LogEvent, LogLevel, ServerCommand, ServerMessage, public_error_code},
    session::{BroadcastHub, CommandDispatch},
};
use crate::obs::{
    client::ObsClient,
    requests,
    state::{ObsSnapshot, ServerStatus},
    validation::extract_resource_names,
};
use crate::server::{client_registry::ClientRegistry, state_store::StateStore};
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// What happened when a freshly dumped config was read back in.
struct DumpReloadOutcome {
    warnings: Vec<String>,
    error: Option<String>,
}

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
    /// Held for as long as a command is reading and rewriting the config file.
    ///
    /// The commands that touch that file are not safe to interleave: a dump
    /// reads the file, merges what OBS reports onto it, writes a backup, and
    /// replaces the file — and two dumps doing that at once could back up a
    /// half-written file or write a merge built from a config that no longer
    /// exists. Nothing used to stop that except the accident that the daemon
    /// ran every command one after another; now that commands from different
    /// clients run at the same time, the exclusion has to be asked for.
    ///
    /// The value is `()` because the thing being guarded is the file on disk,
    /// which Rust cannot hand out a reference to. The lock is always taken
    /// before `config`, never the other way round, so the two cannot deadlock.
    config_file: Mutex<()>,
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
            config_file: Mutex::new(()),
        }
    }

    /// Run one IPC connection's commands, one at a time, until that connection
    /// closes its lane.
    ///
    /// The loop is deliberately serial: it finishes a command and answers it
    /// before it looks at the next one, which is what makes `mute Mic` followed
    /// by `unmute Mic` from the same client happen in that order. It is *this*
    /// loop being shared by every connection that used to make one client's
    /// slow command everybody's wait; `server::command_lanes` now runs one of
    /// these per connection instead, over the same executor.
    pub async fn serve(&self, mut rx: mpsc::Receiver<CommandDispatch>) {
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
            validate_payload(command, &payload.args)?;

            match command {
                ServerCommand::Ping => Ok(json!({ "message": "pong" })),
                ServerCommand::GetServerStatus => self.cmd_server_status().await,
                ServerCommand::GetObsStatus => self.cmd_obs_status().await,
                ServerCommand::GetSnapshot => self.cmd_get_snapshot().await,
                ServerCommand::SetScene => self.cmd_set_scene(&payload.args).await,
                ServerCommand::SetProfile => self.cmd_set_profile(&payload.args).await,
                ServerCommand::SetSceneCollection => {
                    self.cmd_set_scene_collection(&payload.args).await
                }
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

    async fn cmd_server_status(&self) -> Result<Value> {
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

    async fn cmd_obs_status(&self) -> Result<Value> {
        let snap = self.state.read().await;
        Ok(json!({
            "connected": snap.connected,
            "current_scene": snap.current_scene,
            "obs_studio_version": snap.obs_studio_version,
            "obs_websocket_version": snap.obs_websocket_version,
            "last_error": snap.last_error,
        }))
    }

    async fn cmd_get_snapshot(&self) -> Result<Value> {
        let snap = self.state.read().await;
        serde_json::to_value(snap).map_err(|e| ObsctlError::ObsRequestFailed(e.to_string()))
    }

    async fn cmd_set_scene(&self, args: &Value) -> Result<Value> {
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

    async fn cmd_set_profile(&self, args: &Value) -> Result<Value> {
        self.cmd_set_named_selection(args, NamedSelection::PROFILE)
            .await
    }

    async fn cmd_set_scene_collection(&self, args: &Value) -> Result<Value> {
        self.cmd_set_named_selection(args, NamedSelection::SCENE_COLLECTION)
            .await
    }

    /// Switch OBS to a profile or a scene collection.
    ///
    /// Unlike scenes and audio inputs, these two have no aliases or shortcuts,
    /// so the target must already be one of the names in the snapshot. They
    /// differ only in which list is searched, which request is sent, which
    /// not-found error is raised, and what they are called, so those four
    /// live in `selection`.
    async fn cmd_set_named_selection(
        &self,
        args: &Value,
        selection: NamedSelection,
    ) -> Result<Value> {
        let target = required_string(args, "target")?;
        let client = self.require_obs().await?;

        let snap = self.state.read().await;
        let known = (selection.known_names)(&snap).iter().any(|n| n == &target);
        drop(snap);
        if !known {
            // Not `ObsRequestFailed`: no request was made, the name is simply
            // not one OBS knows. Scenes and audio inputs report the same class
            // of mistake through their own not-found codes.
            return Err((selection.not_found)(target));
        }

        client.request((selection.build_request)(&target)?).await?;
        info!("{} set to: {target}", selection.label);
        Ok(json!({ "message": format!("{} set: {target}", selection.lowercase_label) }))
    }

    async fn cmd_set_mute(&self, args: &Value, muted: bool) -> Result<Value> {
        let client = self.require_obs().await?;
        let obs_name = self.resolve_audio_target(args).await?;

        client
            .request(requests::set_input_mute(&obs_name, muted)?)
            .await?;
        let action = if muted { "muted" } else { "unmuted" };
        Ok(json!({ "message": format!("{action}: {obs_name}") }))
    }

    async fn cmd_toggle_mute(&self, args: &Value) -> Result<Value> {
        let client = self.require_obs().await?;
        let obs_name = self.resolve_audio_target(args).await?;

        client
            .request(requests::toggle_input_mute(&obs_name)?)
            .await?;
        Ok(json!({ "message": format!("mute toggled: {obs_name}") }))
    }

    async fn cmd_set_volume(&self, args: &Value) -> Result<Value> {
        let percent = required_u8_percentage(args, "percent")?;
        let client = self.require_obs().await?;
        let obs_name = self.resolve_audio_target(args).await?;

        client
            .request(requests::set_input_volume(
                &obs_name,
                percent_to_mul(percent),
            )?)
            .await?;
        Ok(json!({ "message": format!("volume set to {percent}%: {obs_name}") }))
    }

    /// Turn the caller's `target` — which may be an OBS input name, a
    /// configured alias, or a single-key shortcut — into the name OBS itself
    /// uses. Every audio command starts this way.
    ///
    /// Callers resolve *after* `require_obs`, so that with no OBS connection
    /// the reply is "OBS unavailable" rather than "no such input" — the
    /// snapshot an unreachable daemon holds cannot be trusted to say which
    /// inputs exist.
    async fn resolve_audio_target(&self, args: &Value) -> Result<String> {
        let target = required_string(args, "target")?;
        let snap = self.state.read().await;
        let entries = audio_alias_entries(&snap);
        drop(snap);

        Ok(resolve_audio(&target, &entries)?.name.clone())
    }

    async fn cmd_toggle_stream(&self) -> Result<Value> {
        self.toggle_output(requests::toggle_stream(), "streaming")
            .await
    }

    async fn cmd_toggle_record(&self) -> Result<Value> {
        self.toggle_output(requests::toggle_record(), "recording")
            .await
    }

    /// Flip a stream or recording output and report where it ended up.
    ///
    /// OBS answers a toggle with the resulting `outputActive`, so the reply can
    /// say "started"/"stopped" rather than the ambiguous "toggled" — which is
    /// kept only for the case where OBS sends a reply we cannot read.
    async fn toggle_output(
        &self,
        request: crate::obs::protocol::RequestData,
        output: &str,
    ) -> Result<Value> {
        let client = self.require_obs().await?;
        let result = client.request(request).await?;

        let state = match result.get("outputActive").and_then(|v| v.as_bool()) {
            Some(true) => "started",
            Some(false) => "stopped",
            None => {
                warn!("Malformed toggle response for {output}: missing or invalid outputActive");
                "toggled"
            }
        };

        info!("{output} {state}");
        Ok(json!({ "message": format!("{output} {state}") }))
    }

    /// Write a config file that lists every scene and input OBS currently has,
    /// merged over whatever the user already configured, then reload it.
    async fn cmd_dump_config(&self) -> Result<Value> {
        // Checked first: without a path there is nowhere to write, and the OBS
        // round-trips below would be work thrown away.
        let path = self.config_path.as_ref().ok_or_else(|| {
            ObsctlError::ConfigInvalid("dump-config requires a config file path".to_string())
        })?;

        let obs_resources = self.fetch_dumpable_obs_resources().await?;

        // Taken after the OBS round trips and before the first read of the
        // file: everything from here to the reload is one read-modify-write of
        // the config file and must not be interleaved with another one. The OBS
        // requests are left outside it so a dump does not hold the lock while
        // waiting on the network.
        let _config_file = self.config_file.lock().await;

        let in_memory = self.config.lock().await.clone();
        let base = dump_merge_base(path, &in_memory);
        let merged = dump_config_mod::merge(&base.config, &obs_resources)?;

        let backup = dump_config_mod::write_backup(path)?;
        crate::config::writer::write_atomic(&merged, path)?;
        info!(
            "Config dumped to {} (backup: {})",
            path.display(),
            backup.display()
        );

        let reload = self.reload_after_dump(path).await;

        let scene_count = merged.scenes.len();
        let input_count = merged.audio.inputs.len();
        Ok(json!({
            "message": format!("config dumped: {scene_count} scenes, {input_count} inputs"),
            "merge_base_error": base.error,
            "reload_failed": reload.error.is_some(),
            "warnings": reload.warnings,
            "reload_error": reload.error,
            "scenes": scene_count,
            "inputs": input_count,
        }))
    }

    /// The scene and input names a dumped config should list.
    async fn fetch_dumpable_obs_resources(&self) -> Result<dump_config_mod::ObsResources> {
        let client = self.require_obs().await?;

        let scene_resp = client.request(requests::get_scene_list()).await?;
        let scenes = extract_resource_names(&scene_resp, "scenes", "sceneName")?;

        let input_resp = client.request(requests::get_input_list()).await?;
        let inputs = extract_resource_names(&input_resp, "inputs", "inputName")?;

        Ok(dump_config_mod::ObsResources { scenes, inputs })
    }

    /// Load the file `cmd_dump_config` just wrote back into memory.
    ///
    /// A failure here is reported, not returned: the dump itself succeeded and
    /// the file is on disk, so the caller gets `reload_error` in its result
    /// rather than an error that would imply nothing was written.
    async fn reload_after_dump(&self, path: &std::path::Path) -> DumpReloadOutcome {
        match crate::config::loader::load_with_warnings(path) {
            Ok((new_config, warnings)) => {
                self.log_config_warnings(&warnings, "after dump reload");
                *self.config.lock().await = new_config;
                DumpReloadOutcome {
                    warnings: Self::warning_messages(&warnings),
                    error: None,
                }
            }
            Err(e) => {
                warn!("Failed to reload config after dump: {e}");
                self.publish_log(
                    LogLevel::Warn,
                    format!("Config reload after dump failed: {e}"),
                );
                DumpReloadOutcome {
                    warnings: Vec::new(),
                    error: Some(e.to_string()),
                }
            }
        }
    }

    async fn cmd_validate_config(&self) -> Result<Value> {
        let config = self.config.lock().await;
        let warnings = crate::config::schema::validate(&config)?;
        let warning_msgs: Vec<String> = warnings.iter().map(|w| w.0.clone()).collect();
        Ok(json!({ "valid": true, "warnings": warning_msgs }))
    }

    async fn cmd_reload_config(&self) -> Result<Value> {
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

    async fn reload_config_from_disk(&self) -> Result<Vec<ValidationWarning>> {
        let path = self.config_path.as_ref().ok_or_else(|| {
            ObsctlError::ConfigInvalid("no config path configured for reload".to_string())
        })?;

        // The same lock a dump takes: a reload reads the file, so it must not
        // read it while a dump is part-way through replacing it.
        let _config_file = self.config_file.lock().await;

        let (new_config, warnings) = crate::config::loader::load_with_warnings(path)?;
        self.log_config_warnings(&warnings, "on reload");

        let scenes = new_config.scenes.clone();
        let audio_inputs = new_config.audio.inputs.clone();

        {
            let mut guard = self.config.lock().await;
            *guard = new_config;
        }

        // `merge_config` broadcasts the updated alias/shortcut metadata itself.
        self.state.merge_config(&scenes, &audio_inputs).await;

        info!("Config reloaded from {}", path.display());
        Ok(warnings)
    }

    /// Ask the supervisor to drop the current OBS connection and dial again.
    ///
    /// The request travels over a channel whose receiving end lives inside
    /// `ObsSupervisor::run`. When that function returns, the receiver is
    /// dropped and nothing will ever act on another request — which is not
    /// hypothetical: with `reconnect.enabled: false` in the config the
    /// supervisor stops after its first failed connect, and the daemon cannot
    /// reach OBS again for the rest of its life. Reporting the failed send as
    /// success meant every later `obsctl reconnect` printed a confirmation,
    /// exited 0, and did nothing at all.
    async fn cmd_reconnect_obs(&self) -> Result<Value> {
        self.reconnect_tx
            .send(())
            .await
            .map_err(|_| ObsctlError::ObsUnavailable)?;
        Ok(json!({ "message": "reconnect triggered" }))
    }

    async fn cmd_shutdown_server(&self) -> Result<Value> {
        let config = self.config.lock().await;
        if !config.server.allow_remote_shutdown {
            return Err(ObsctlError::ShutdownDisabled);
        }
        drop(config);
        info!("Shutdown requested via IPC");
        self.publish_log(LogLevel::Info, "Shutdown requested via IPC");
        // Same reasoning as `cmd_reconnect_obs`: a send that nothing is left to
        // receive means the daemon will not act on this, so the caller must not
        // be told it worked. `ObsUnavailable` is reused rather than inventing an
        // error code, because a new one would be a change to the public IPC
        // contract (error code, exit code, README table, CLI mapping) for a
        // case that only arises while the daemon is already on its way down.
        self.shutdown_tx
            .send(true)
            .map_err(|_| ObsctlError::ObsUnavailable)?;
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

    async fn require_obs(&self) -> Result<ObsClient> {
        self.obs
            .lock()
            .await
            .clone()
            .ok_or(ObsctlError::ObsUnavailable)
    }
}

/// The config a dump merges OBS's current state onto, and why it might not be
/// the one on disk.
struct DumpMergeBase {
    config: Config,
    /// `None` when the file was read. Otherwise why it was not, so the caller
    /// can say the dump was built from a possibly-stale in-memory copy.
    error: Option<String>,
}

/// Choose the config that a `dump-config` should merge onto.
///
/// The file, not the daemon's in-memory copy. The daemon loads the config at
/// startup and on `reload-config`; a user who hand-edits the file and then runs
/// `dump-config` — which the README's quick start puts one line apart — would
/// otherwise have their edits overwritten by a copy that predates them.
///
/// A file that cannot be read or does not parse falls back to the in-memory
/// copy rather than failing. The dump's job is to record what OBS has, and
/// refusing to do it because the file on disk is currently broken would take
/// away the command most likely to produce a working one. The caller reports
/// the fallback.
fn dump_merge_base(path: &std::path::Path, in_memory: &Config) -> DumpMergeBase {
    match crate::config::loader::load_with_warnings(path) {
        Ok((config, _)) => DumpMergeBase {
            config,
            error: None,
        },
        Err(error) => {
            warn!(
                "Dumping onto the in-memory config; {} is unusable: {error}",
                path.display()
            );
            DumpMergeBase {
                config: in_memory.clone(),
                error: Some(error.to_string()),
            }
        }
    }
}

/// Reject a payload whose shape does not match what the command declares in
/// `ipc::protocol::COMMANDS`, before any OBS request is attempted.
fn validate_payload(command: ServerCommand, args: &Value) -> Result<()> {
    match command.required_args() {
        [] => validate_empty_payload(args, command.name()),
        required => validate_object_args(args, command.name(), required),
    }
}

#[cfg(test)]
fn validate_command_payload(command: &str, args: &Value) -> Result<()> {
    validate_payload(parse_server_command(command)?, args)
}

fn parse_server_command(name: &str) -> Result<ServerCommand> {
    ServerCommand::parse(name)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("unknown command: {name}")))
}

fn required_string(args: &Value, key: &str) -> Result<String> {
    let raw = args
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))?;

    trim_and_validate_token_with_max_len(&raw, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::CommandParseError(format!("{key} {error}")))
}

fn required_u8_percentage(args: &Value, key: &str) -> Result<u8> {
    let value = args
        .get(key)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))?;

    // `as_u64` rejects negatives and non-integers (including 50.5) for us, so
    // the only remaining question is the 0-100 range.
    match value.as_u64() {
        Some(percent) if percent <= 100 => Ok(percent as u8),
        _ => Err(ObsctlError::CommandParseError(format!(
            "{key} must be an integer 0-100"
        ))),
    }
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

/// The parts that differ between "set the current profile" and "set the
/// current scene collection": where the known names live in the snapshot, how
/// to build the OBS request, which error says the name is unknown, and how to
/// name the thing in a message.
struct NamedSelection {
    known_names: fn(&ObsSnapshot) -> &[String],
    build_request: fn(&str) -> Result<crate::obs::protocol::RequestData>,
    not_found: fn(String) -> ObsctlError,
    label: &'static str,
    lowercase_label: &'static str,
}

impl NamedSelection {
    const PROFILE: Self = Self {
        known_names: |snap| &snap.profiles,
        build_request: requests::set_current_profile,
        not_found: ObsctlError::ProfileNotFound,
        label: "Profile",
        lowercase_label: "profile",
    };

    const SCENE_COLLECTION: Self = Self {
        known_names: |snap| &snap.scene_collections,
        build_request: requests::set_current_scene_collection,
        not_found: ObsctlError::SceneCollectionNotFound,
        label: "Scene collection",
        lowercase_label: "scene collection",
    };
}

/// Alias/shortcut lookup tables for the two kinds of target a command can
/// name. `resolve` works off `AliasEntry`, so both snapshot lists are projected
/// into it the same way.
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
    use super::{
        MAX_TARGET_TOKEN_LENGTH, parse_server_command, required_string, required_u8_percentage,
    };
    use super::{validate_command_payload, validate_empty_payload, validate_object_args};
    use serde_json::json;

    /// `dump-config` must merge onto the file, not onto the copy the daemon
    /// loaded at startup — otherwise a hand-edit made since then is silently
    /// overwritten by the dump.
    #[test]
    fn dump_merge_base_prefers_the_file_over_the_in_memory_copy() {
        use crate::config::model::{Config, SceneConfig};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");

        // What the user just wrote by hand.
        let on_disk = Config {
            scenes: vec![SceneConfig {
                name: "Main".to_string(),
                alias: Some("edited-by-hand".to_string()),
                ..SceneConfig::default()
            }],
            ..Config::default()
        };
        crate::config::writer::write_atomic(&on_disk, &path).unwrap();

        // What the daemon still holds from startup.
        let in_memory = Config::default();

        let base = super::dump_merge_base(&path, &in_memory);
        assert!(base.error.is_none(), "the file was readable");
        assert_eq!(
            base.config.scenes.first().and_then(|s| s.alias.as_deref()),
            Some("edited-by-hand"),
            "the dump must build on what is on disk"
        );
    }

    /// A config file that is missing or unparseable does not fail the dump:
    /// recording what OBS has is exactly what someone with a broken config
    /// needs. The caller is told which copy was used.
    #[test]
    fn dump_merge_base_falls_back_to_memory_when_the_file_is_unusable() {
        use crate::config::model::{Config, SceneConfig};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-valid.yml");
        std::fs::write(&path, "this: [is not: valid yaml").unwrap();

        let in_memory = Config {
            scenes: vec![SceneConfig {
                name: "FromMemory".to_string(),
                ..SceneConfig::default()
            }],
            ..Config::default()
        };

        let base = super::dump_merge_base(&path, &in_memory);
        assert!(base.error.is_some(), "the fallback must be reported");
        assert_eq!(
            base.config.scenes.first().map(|s| s.name.as_str()),
            Some("FromMemory")
        );
    }

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

    /// Every command name and its argument shape live in
    /// `ipc::protocol::COMMANDS`, which owns the name round-trip test. The
    /// executor's own job is to turn a name that is not in that table into a
    /// parse error rather than a panic.
    #[test]
    fn parse_server_command_rejects_unknown_name() {
        assert!(parse_server_command("does-not-exist").is_err());
    }
}
