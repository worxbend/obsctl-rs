use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::config::{
    dump as dump_config_mod,
    loader::load_with_warnings,
    model::{Config, SceneProfileConfig},
    schema::{ValidationWarning, validate},
    writer::write_atomic,
};
use crate::domain::{
    aliases::{resolve, resolve_audio},
    errors::ObsctlError,
    names::normalized_name,
    result::Result,
    scene_profiles::{ActiveSceneProfile, SceneVisibility},
    volume::percent_to_mul,
};
use crate::ipc::{
    protocol::{
        CommandPayload, DeleteSceneProfileResult, DumpConfigResult, ErrorPayload, RENAME_FROM,
        SaveSceneProfileResult, SceneProfileEntry, SceneProfileListing, ServerCommand,
        ServerMessage, SetSceneProfileResult, public_error_code,
    },
    session::{BroadcastHub, CommandDispatch},
};
use crate::obs::{
    client::ObsClient,
    protocol::RequestData,
    requests,
    state::{ObsSnapshot, ServerStatus},
    validation::extract_resource_names,
};
use crate::server::{
    client_registry::ClientRegistry,
    command_args::{
        optional_string, parse_server_command, required_string, required_string_array,
        required_u8_percentage, validate_payload,
    },
    log_relay::ServerLog,
    state_store::{ConfigProjection, StateStore},
};

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
    log: ServerLog,
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
            log: ServerLog::new(cfg.hub, "obsctl_rs::server::command_executor"),
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

    async fn handle(&self, id: String, payload: CommandPayload) -> ServerMessage {
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
                ServerCommand::SetSceneProfile => self.cmd_set_scene_profile(&payload.args).await,
                ServerCommand::ClearSceneProfile => self.cmd_clear_scene_profile().await,
                ServerCommand::SaveSceneProfile => self.cmd_save_scene_profile(&payload.args).await,
                ServerCommand::DeleteSceneProfile => {
                    self.cmd_delete_scene_profile(&payload.args).await
                }
                ServerCommand::ListSceneProfiles => self.cmd_list_scene_profiles().await,
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
        let snap = self.state.snapshot().await;
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
        // Not an OBS request at all. The variant is reused deliberately, for
        // the reason spelled out in `cmd_shutdown_server`: a truthful new one
        // would be a change to the public IPC contract.
        serde_json::to_value(status).map_err(|e| {
            ObsctlError::ObsRequestFailed(format!("failed to serialize server status: {e}"))
        })
    }

    async fn cmd_obs_status(&self) -> Result<Value> {
        let snap = self.state.snapshot().await;
        Ok(json!({
            "connected": snap.connected,
            "current_scene": snap.current_scene,
            "obs_studio_version": snap.obs_studio_version,
            "obs_websocket_version": snap.obs_websocket_version,
            "last_error": snap.last_error,
        }))
    }

    async fn cmd_get_snapshot(&self) -> Result<Value> {
        let snap = self.state.snapshot().await;
        // Reusing `ObsRequestFailed` for a local failure, as in
        // `cmd_server_status` above.
        serde_json::to_value(snap).map_err(|e| {
            ObsctlError::ObsRequestFailed(format!("failed to serialize snapshot: {e}"))
        })
    }

    async fn cmd_set_scene(&self, args: &Value) -> Result<Value> {
        let target = required_string(args, "target")?;
        let client = self.require_obs().await?;
        let entries = self.state.scene_aliases().await;

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

        let snap = self.state.snapshot().await;
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
        let entries = self.state.audio_aliases().await;

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
    async fn toggle_output(&self, request: RequestData, output: &str) -> Result<Value> {
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
        let base = config_write_base(path, &in_memory);
        let merged = dump_config_mod::merge(&base.config, &obs_resources)?;

        let backup = dump_config_mod::write_backup(path)?;
        write_atomic(&merged, path)?;
        info!(
            "Config dumped to {} (backup: {})",
            path.display(),
            backup.display()
        );

        let reload = self.reload_after_dump(path).await;

        let scene_count = merged.scenes.len();
        let input_count = merged.audio.inputs.len();
        let result = DumpConfigResult {
            message: format!("config dumped: {scene_count} scenes, {input_count} inputs"),
            merge_base_error: base.error,
            reload_failed: reload.error.is_some(),
            warnings: reload.warnings,
            reload_error: reload.error,
            scenes: scene_count,
            inputs: input_count,
        };
        serialize_result(result, "dump-config result")
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
        match load_with_warnings(path) {
            Ok((new_config, warnings)) => {
                self.log_config_warnings(&warnings, "after dump reload");
                *self.config.lock().await = new_config;
                DumpReloadOutcome {
                    warnings: Self::warning_messages(&warnings),
                    error: None,
                }
            }
            Err(e) => {
                self.log
                    .warn(format!("Config reload after dump failed: {e}"));
                DumpReloadOutcome {
                    warnings: Vec::new(),
                    error: Some(e.to_string()),
                }
            }
        }
    }

    /// Read the config file, apply `edit`, validate the result, write it back,
    /// and make the daemon and every subscriber agree with what is now on disk.
    ///
    /// The ordering is the whole point of the helper. Nothing is applied in
    /// memory until the file has been written, so a rejected edit or a failed
    /// write leaves the daemon holding exactly what the file still says
    /// instead of the two disagreeing until the next reload.
    ///
    /// There is deliberately no `.bak` copy. Backups belong to `dump-config`,
    /// which rewrites the whole file from what OBS reports; a scene-profile
    /// edit changes two sections, and writing a backup for each one would
    /// litter the config directory as fast as the user presses save. That is
    /// also why a file that cannot be read is refused below instead of being
    /// written over: with no backup, there would be nothing to restore from.
    ///
    /// Like `dump-config`, this rewrites the file from the parsed model, so
    /// comments and key ordering in a hand-written config do not survive it.
    async fn edit_config_file<F>(&self, edit: F) -> Result<(Config, Vec<ValidationWarning>)>
    where
        F: FnOnce(&mut Config) -> Result<()>,
    {
        // Checked first: with nowhere to write, the edit cannot be made
        // durable, and applying it in memory only would be a change the user
        // loses at the next restart without being told.
        let path = self.config_path.as_ref().ok_or_else(|| {
            ObsctlError::ConfigInvalid("scene profiles require a config file path".to_string())
        })?;

        // The same lock `dump-config` and `reload-config` take: everything
        // from the read below to the write is one read-modify-write of the
        // file and must not interleave with another one.
        let _config_file = self.config_file.lock().await;

        let in_memory = self.config.lock().await.clone();
        let base = config_write_base(path, &in_memory);
        // A config file that exists but cannot be read or parsed is not
        // something to build a write on. `dump-config` can fall back to the
        // daemon's in-memory copy because it takes a backup first and its
        // whole job is to produce a working file out of what OBS reports; a
        // scene-profile edit takes no backup, so the same fallback here would
        // replace a file — every hand edit made since the daemon started
        // included — with a copy that predates them, and report success.
        // Refusing leaves the file untouched and tells the user what to fix.
        // A file that is merely *absent* is a different matter: writing it
        // from memory recreates what the daemon is already running on.
        if let Some(error) = base.error.filter(|_| !base.missing) {
            return Err(ObsctlError::ConfigInvalid(format!(
                "refusing to overwrite {}, which cannot be read: {error}",
                path.display()
            )));
        }

        let before = base.config;
        let mut edited = before.clone();
        edit(&mut edited)?;

        let warnings = validate(&edited)?;
        // An edit that landed on the config the file already holds has
        // nothing to write. Activating the profile that is already active and
        // clearing when nothing is active are both legitimate requests — the
        // caller asked for a state and that state is already reached — but
        // rewriting the file for them re-serializes it, moves its
        // modification time, and wakes anything watching it for a change that
        // did not happen. Everything below still runs: the daemon's in-memory
        // copy is compared against nothing here, only the file is, so the
        // swap and the broadcast are what bring memory back in line with disk
        // after a hand edit.
        if config_file_would_change(&before, &edited) {
            write_atomic(&edited, path)?;
        }

        {
            let mut guard = self.config.lock().await;
            *guard = edited.clone();
        }

        // What turns the edit into something clients can see: the projection
        // re-resolves which scenes are hidden, and `merge_config` broadcasts
        // the updated snapshot on the `state` topic.
        let projection = ConfigProjection::from_config(&edited);
        self.state.merge_config(&projection).await;

        self.log_config_warnings(&warnings, "after scene profile change");

        Ok((edited, warnings))
    }

    /// Switch a scene profile on. The per-scene `hidden` flags stop deciding
    /// anything until it is switched off again.
    async fn cmd_set_scene_profile(&self, args: &Value) -> Result<Value> {
        let target = required_string(args, "target")?;

        let mut name = String::new();
        let mut listed = Vec::new();
        let (_, warnings) = self
            .edit_config_file(|config| {
                // The stored spelling, not the caller's: the config file
                // should keep naming the profile the way it names it in the
                // `scene_profiles` list above.
                let profile = find_scene_profile(config, &target)
                    .ok_or_else(|| scene_profile_not_found(&target))?;
                name = profile.name.clone();
                listed = profile.hidden.clone();
                config.active_scene_profile = Some(name.clone());
                Ok(())
            })
            .await?;

        // The scenes that will really disappear, not the entries the file
        // lists — see [`scenes_hidden_by`].
        let hidden = scenes_hidden_by(&self.state.snapshot().await, &listed);
        info!("Scene profile set to: {name}");

        serialize_result(
            SetSceneProfileResult {
                message: format!("scene profile set: {name}"),
                hidden,
                listed: listed.len(),
                warnings: Self::warning_messages(&warnings),
            },
            "scene profile set result",
        )
    }

    /// Switch off whatever scene profile is active, handing the per-scene
    /// `hidden` flags back their say. Clearing when nothing is active is not
    /// an error — the caller asked for a state, and that state is reached.
    async fn cmd_clear_scene_profile(&self) -> Result<Value> {
        self.edit_config_file(|config| {
            config.active_scene_profile = None;
            Ok(())
        })
        .await?;

        info!("Scene profile cleared");
        Ok(json!({ "message": "scene profile cleared" }))
    }

    /// Create a scene profile, replace one that already exists, or rename one.
    ///
    /// `rename_from` is what makes the third of those possible in a single
    /// command. Without it a client renaming a profile had to save under the
    /// new name and then delete the old one, which rewrote the config file
    /// twice and — because deleting the active profile switches it off, which
    /// is the right thing for a delete the user actually asked for — silently
    /// stopped hiding any scenes whenever the profile being renamed was the
    /// one in effect. With it the entry is moved where it stands, keeping its
    /// place in the list and, below, its hold on `active_scene_profile`.
    ///
    /// Saving otherwise never changes which profile is active: the TUI's
    /// editor lets a user work on a profile they are not currently using, and
    /// having the save switch the scene list out from under them would be a
    /// surprise.
    async fn cmd_save_scene_profile(&self, args: &Value) -> Result<Value> {
        let name = required_string(args, "target")?;
        let hidden = required_string_array(args, "hidden")?;
        let rename_from = optional_string(args, RENAME_FROM)?;

        let mut created = false;
        let mut renamed = false;
        let (config, warnings) = self
            .edit_config_file(|config| {
                let profile = SceneProfileConfig {
                    name: name.clone(),
                    hidden: hidden.clone(),
                };
                // The entry being replaced, when the caller named one, and the
                // entry the new name would land on. A rename is interesting
                // only when those are two different entries.
                let replacing = rename_from
                    .as_deref()
                    .and_then(|previous| scene_profile_position(config, previous));
                let by_name = scene_profile_position(config, &name);

                match (replacing, by_name) {
                    // Renaming onto a name a *different* profile answers to
                    // would destroy that profile: this save would overwrite
                    // it, and the entry being renamed away from would go too.
                    // Two profiles collapsing into one is not something a user
                    // can undo — `edit_config_file` writes no backup — so it
                    // is refused rather than done quietly.
                    (Some(from), Some(other)) if from != other => {
                        return Err(scene_profile_name_taken(&name));
                    }
                    (Some(from), _) => {
                        let previous = config.scene_profiles[from].name.clone();
                        renamed = !same_scene_profile_name(&previous, &name);
                        let was_active = config
                            .active_scene_profile
                            .as_deref()
                            .is_some_and(|active| same_scene_profile_name(active, &previous));
                        config.scene_profiles[from] = profile;
                        // `active_scene_profile` points by name, so renaming
                        // the profile in effect has to move the pointer with
                        // it or the config would name a profile that is no
                        // longer there.
                        if was_active {
                            config.active_scene_profile = Some(name.clone());
                        }
                    }
                    // Not a rename: create, or replace whole a profile of the
                    // same name — spelling included, since the name the caller
                    // just gave is the one they meant even when it differs
                    // from the stored one only in case.
                    (None, Some(index)) => config.scene_profiles[index] = profile,
                    (None, None) => {
                        created = true;
                        config.scene_profiles.push(profile);
                    }
                }
                Ok(())
            })
            .await?;

        // Whether the profile just written is the one in effect. A save never
        // switches a profile on, but it can very much change what a profile
        // that is *already* on hides — the projection above re-resolved the
        // scene list from it before this line ran — so a client that told the
        // user "the active profile is unchanged" while the dashboard visibly
        // lost rows would be describing the wrong event. Answered from the
        // config the edit produced rather than from the caller's idea of what
        // was active, which the same edit may have just moved (a rename of the
        // active profile carries `active_scene_profile` with it).
        let active = config
            .active_scene_profile
            .as_deref()
            .is_some_and(|active| same_scene_profile_name(active, &name));
        // The scenes that will really disappear, not the entries the file
        // lists — see [`scenes_hidden_by`].
        let effective = scenes_hidden_by(&self.state.snapshot().await, &hidden);

        info!("Scene profile saved: {name}");
        serialize_result(
            SaveSceneProfileResult {
                message: format!("scene profile saved: {name}"),
                hidden: effective,
                listed: hidden.len(),
                created,
                renamed,
                active,
                warnings: Self::warning_messages(&warnings),
            },
            "scene profile save result",
        )
    }

    /// Remove a scene profile.
    ///
    /// Deleting the active one also switches it off. Leaving
    /// `active_scene_profile` pointing at a profile that no longer exists
    /// would make every later validation report a warning about a mistake the
    /// user never made.
    async fn cmd_delete_scene_profile(&self, args: &Value) -> Result<Value> {
        let target = required_string(args, "target")?;

        let mut deleted_name = String::new();
        let mut deactivated = false;
        self.edit_config_file(|config| {
            let index = scene_profile_position(config, &target)
                .ok_or_else(|| scene_profile_not_found(&target))?;
            let removed = config.scene_profiles.remove(index);
            deleted_name = removed.name.clone();

            let was_active = config
                .active_scene_profile
                .as_deref()
                .is_some_and(|active| same_scene_profile_name(active, &removed.name));
            if was_active {
                config.active_scene_profile = None;
                deactivated = true;
            }
            Ok(())
        })
        .await?;

        info!("Scene profile deleted: {deleted_name}");
        serialize_result(
            DeleteSceneProfileResult {
                message: format!("scene profile deleted: {deleted_name}"),
                deactivated,
            },
            "scene profile delete result",
        )
    }

    /// What scene profiles the daemon is holding, and which one is on.
    ///
    /// Answered from memory. Unlike the four commands above this one changes
    /// nothing, so it neither reads the file nor takes the file lock — a
    /// listing must not queue behind a save that is part-way through writing.
    async fn cmd_list_scene_profiles(&self) -> Result<Value> {
        let config = self.config.lock().await;
        let active = config.active_scene_profile().map(|p| p.name.clone());
        let profiles: Vec<SceneProfileEntry> = config
            .scene_profiles
            .iter()
            .map(|profile| SceneProfileEntry {
                name: profile.name.clone(),
                hidden: profile.hidden.clone(),
            })
            .collect();
        drop(config);

        serialize_result(
            SceneProfileListing { active, profiles },
            "scene profile listing",
        )
    }

    async fn cmd_validate_config(&self) -> Result<Value> {
        let config = self.config.lock().await;
        let warnings = validate(&config)?;
        let warning_msgs: Vec<String> = warnings.iter().map(|w| w.0.clone()).collect();
        Ok(json!({ "valid": true, "warnings": warning_msgs }))
    }

    async fn cmd_reload_config(&self) -> Result<Value> {
        let result = self.reload_config_from_disk().await;
        match &result {
            Ok(warnings) => {
                // The wire text stays "Config reloaded" — clients match on it —
                // even though the process log used to name the path here.
                self.log.info("Config reloaded");
                let warning_count = warnings.len();
                if warning_count > 0 {
                    self.log
                        .info(format!("Config reloaded with {warning_count} warning(s)"));
                }
            }
            Err(e) => self.log.warn(format!("Config reload failed: {e}")),
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

        let (new_config, warnings) = load_with_warnings(path)?;
        self.log_config_warnings(&warnings, "on reload");

        let projection = ConfigProjection::from_config(&new_config);

        {
            let mut guard = self.config.lock().await;
            *guard = new_config;
        }

        // `merge_config` broadcasts the updated metadata itself. That includes
        // which scenes are hidden, so hand-editing `active_scene_profile:` and
        // running `reload-config` re-hides the scene list without a restart.
        self.state.merge_config(&projection).await;

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
        self.log.info("Shutdown requested via IPC");
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

    fn log_config_warnings(&self, warnings: &[ValidationWarning], context: &str) {
        for warning in warnings {
            self.log
                .warn(format!("Config warning {context}: {}", warning.0));
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

/// The config a command about to rewrite the config file starts from, and why
/// it might not be the one on disk.
struct ConfigWriteBase {
    config: Config,
    /// `None` when the file was read. Otherwise why it was not, so the caller
    /// can say the write was built from a possibly-stale in-memory copy.
    error: Option<String>,
    /// Whether the reason was that there is no file there yet, as opposed to
    /// one that is there but unusable. `dump-config` treats the two the same;
    /// `edit_config_file` does not, because only one of them means a write
    /// would destroy something.
    missing: bool,
}

/// Choose the config that a command rewriting the config file should build on.
///
/// The file, not the daemon's in-memory copy. The daemon loads the config at
/// startup and on `reload-config`; a user who hand-edits the file and then runs
/// `dump-config` — which the README's quick start puts one line apart — would
/// otherwise have their edits overwritten by a copy that predates them. Saving
/// a scene profile has the same hazard for the same reason.
///
/// A file that cannot be read or does not parse falls back to the in-memory
/// copy rather than failing. A dump's job is to record what OBS has, and
/// refusing to do it because the file on disk is currently broken would take
/// away the command most likely to produce a working one. The caller reports
/// the fallback.
fn config_write_base(path: &std::path::Path, in_memory: &Config) -> ConfigWriteBase {
    match load_with_warnings(path) {
        Ok((config, _)) => ConfigWriteBase {
            config,
            error: None,
            missing: false,
        },
        Err(error) => {
            warn!(
                "Writing onto the in-memory config; {} is unusable: {error}",
                path.display()
            );
            ConfigWriteBase {
                config: in_memory.clone(),
                missing: matches!(error, ObsctlError::ConfigNotFound(_)),
                error: Some(error.to_string()),
            }
        }
    }
}

/// Whether writing `after` would leave a config file different from the one
/// `before` was read out of.
///
/// The comparison is made on the YAML the writer would produce rather than on
/// the two models, for two reasons. `Config` holds the connection secrets and
/// deliberately derives neither `Debug` nor `PartialEq`, so there is no field
/// comparison to make; and the rendered text is the thing a write would
/// actually replace, which is exactly the question being asked.
///
/// A model that will not serialize answers `true`: the write then runs and
/// fails with the real error, instead of this helper swallowing the edit and
/// reporting success.
fn config_file_would_change(before: &Config, after: &Config) -> bool {
    match (serde_yaml::to_string(before), serde_yaml::to_string(after)) {
        (Ok(before), Ok(after)) => before != after,
        _ => true,
    }
}

/// How many of `hidden` name a scene the daemon has actually seen.
///
/// A profile's `hidden` list is config, and config outlives the scenes it
/// names: rename a scene in OBS and the old spelling stays in the file, hiding
/// nothing. Counting the entries would promise the user more rows than are
/// going to disappear, and would disagree with the TUI's own badge, which
/// counts the rows that really went. Duplicate spellings of one scene collapse
/// here too, because [`SceneVisibility`] holds a normalized set.
///
/// `None` means the daemon has no scene list to check against — it has not
/// finished talking to OBS yet — and there is then no better answer than the
/// number of entries the profile lists.
/// Turn a typed command reply into the JSON the daemon puts on the wire.
///
/// Reusing `ObsRequestFailed` for a local serialization failure, as
/// `cmd_server_status` does: a truthful new error variant would be a change to
/// the public IPC contract.
fn serialize_result<T: serde::Serialize>(result: T, label: &str) -> Result<Value> {
    serde_json::to_value(result)
        .map_err(|e| ObsctlError::ObsRequestFailed(format!("failed to serialize {label}: {e}")))
}

fn scenes_hidden_by(snapshot: &ObsSnapshot, hidden: &[String]) -> usize {
    if snapshot.scenes.is_empty() {
        return hidden.len();
    }
    let visibility =
        SceneVisibility::resolve(std::iter::empty(), ActiveSceneProfile::Named(hidden));
    snapshot
        .scenes
        .iter()
        .filter(|scene| visibility.is_hidden(&scene.name))
        .count()
}

/// Whether two spellings name the same scene profile.
///
/// Case- and whitespace-insensitive, the same rule
/// [`Config::active_scene_profile`] matches by, so `obsctl scene-profile
/// STREAMING` finds the profile the user wrote as `streaming`. A name that is
/// not usable at all matches nothing, including another unusable one.
fn same_scene_profile_name(left: &str, right: &str) -> bool {
    match (normalized_name(left), normalized_name(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// The scene profile in `config` that `target` names.
fn find_scene_profile<'a>(config: &'a Config, target: &str) -> Option<&'a SceneProfileConfig> {
    config
        .scene_profiles
        .iter()
        .find(|profile| same_scene_profile_name(&profile.name, target))
}

/// Where the scene profile `target` names sits in `config.scene_profiles`.
fn scene_profile_position(config: &Config, target: &str) -> Option<usize> {
    config
        .scene_profiles
        .iter()
        .position(|profile| same_scene_profile_name(&profile.name, target))
}

/// A scene profile the config does not define.
///
/// `ConfigInvalid` rather than a new error variant: the config file is the
/// only place scene profiles exist, so "there is no such profile" is a
/// statement about the config. A truthful new variant would mean a new public
/// error code, a new CLI exit code and a new README row for a case the
/// existing taxonomy already describes.
fn scene_profile_not_found(target: &str) -> ObsctlError {
    ObsctlError::ConfigInvalid(format!("scene profile not found: {target}"))
}

/// A rename that would land on a name another scene profile already answers
/// to. `ConfigInvalid` for the same reason as above: it is a statement about
/// what the config file already contains.
fn scene_profile_name_taken(name: &str) -> ObsctlError {
    ObsctlError::ConfigInvalid(format!("scene profile already exists: {name}"))
}

/// The parts that differ between "set the current profile" and "set the
/// current scene collection": where the known names live in the snapshot, how
/// to build the OBS request, which error says the name is unknown, and how to
/// name the thing in a message.
struct NamedSelection {
    known_names: fn(&ObsSnapshot) -> &[String],
    build_request: fn(&str) -> Result<RequestData>,
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

#[cfg(test)]
mod tests {

    /// `dump-config` must merge onto the file, not onto the copy the daemon
    /// loaded at startup — otherwise a hand-edit made since then is silently
    /// overwritten by the dump.
    #[test]
    fn config_write_base_prefers_the_file_over_the_in_memory_copy() {
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

        let base = super::config_write_base(&path, &in_memory);
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
    fn config_write_base_falls_back_to_memory_when_the_file_is_unusable() {
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

        let base = super::config_write_base(&path, &in_memory);
        assert!(base.error.is_some(), "the fallback must be reported");
        assert_eq!(
            base.config.scenes.first().map(|s| s.name.as_str()),
            Some("FromMemory")
        );
    }
}
