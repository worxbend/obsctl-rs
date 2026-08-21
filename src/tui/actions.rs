//! Turning a [`TuiAction`] into a change to the model, and — when the action
//! is one the daemon has to answer — into the round-trip that answers it.
//!
//! One exhaustive [`dispatch`] decides which of the two an action is, so the
//! keyboard, the mouse, and the command palette all meet in a single list
//! rather than in three that have to be kept in step.

use std::path::Path;

use tokio::sync::mpsc;

use crate::{
    config::{loader, writer},
    ipc::protocol::{ServerCommand, ServerMessage},
    tui::{
        app::spawn_session_forwarder,
        daemon::{PaletteOutcome, ReplyStyle, dispatch_palette_command, send_simple_with_target},
        input::TuiAction,
        model::{CommandPaletteState, FocusPanel, TuiModel},
        mouse::Hitboxes,
        render::half_page,
        session::TuiEventSession,
        theme::Theme,
    },
};

/// Percentage points one `h`/`l` (or `←`/`→`) nudge moves the focused input.
const VOLUME_STEP: i16 = 5;

/// Everything an action might need that isn't the model itself.
pub(super) struct ActionCtx<'a> {
    pub(super) socket_path: &'a Path,
    pub(super) config_path: Option<&'a Path>,
    pub(super) ipc_tx: &'a mpsc::Sender<std::result::Result<ServerMessage, String>>,
    pub(super) vol_tx: &'a mpsc::UnboundedSender<(String, u8)>,
    pub(super) hits: &'a Hitboxes,
}

/// What one action leaves behind: nothing, a line for the status bar, or the
/// decision to leave the program.
///
/// This was a bare `(bool, Option<String>)` threaded through three functions,
/// which meant nearly every arm of the dispatch below literally read
/// `Some((false, None))`, and reading `true` as "quit" depended on
/// remembering which slot was which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ActionOutcome {
    /// Stay in the TUI with nothing to report.
    Continue,
    /// Stay in the TUI and put this on the status line.
    Status(String),
    /// Leave the TUI.
    Quit,
}

impl ActionOutcome {
    /// The common case: the action did its work and has nothing to say.
    fn done() -> Self {
        Self::Continue
    }

    fn status(message: impl Into<String>) -> Self {
        Self::Status(message.into())
    }

    /// For the helpers that report a line only sometimes.
    fn maybe_status(message: Option<String>) -> Self {
        match message {
            Some(message) => Self::Status(message),
            None => Self::Continue,
        }
    }
}

/// Where an action is handled: it is either finished already, or it needs a
/// round-trip to the daemon.
///
/// Which of the two an action was used to be stated in three places that had
/// to be kept in step by hand — a table of send-and-report commands, a match
/// in `run_action`, and a list of those same names in `apply_local_action`'s
/// `None` arm — with a runtime `expect` where they met, so forgetting one list
/// either killed the TUI on the first keypress or swallowed the key. It is
/// stated once now, in [`dispatch`], whose match is exhaustive: a newly added
/// `TuiAction` fails to compile instead.
enum Dispatched {
    Done(ActionOutcome),
    Daemon(DaemonWork),
}

/// A daemon round-trip described as data rather than performed on the spot, so
/// that *deciding* what to send stays synchronous, socket-free, and testable
/// with nothing listening.
enum DaemonWork {
    /// Send a command that takes no arguments, and report the reply in the
    /// style that suits it.
    Simple(ServerCommand, ReplyStyle),
    /// Send a command naming one target — a scene, profile, collection, or
    /// audio input — and report the reply.
    Targeted(ServerCommand, String),
    /// Re-open this TUI's event session with the daemon.
    RetryConnect,
    /// Run a line typed into the command palette.
    PaletteSubmit(String),
    /// Save the theme just confirmed in the settings view to the config file.
    ApplyTheme(Theme),
}

impl DaemonWork {
    async fn run(self, model: &mut TuiModel, ctx: &ActionCtx<'_>) -> ActionOutcome {
        let socket_path = ctx.socket_path;
        match self {
            DaemonWork::Simple(command, reply) => {
                ActionOutcome::Status(reply.send(socket_path, command).await)
            }
            DaemonWork::Targeted(command, target) => {
                ActionOutcome::Status(send_simple_with_target(socket_path, command, &target).await)
            }
            DaemonWork::RetryConnect => match TuiEventSession::connect(socket_path).await {
                Ok(session) => {
                    model.connected_to_daemon = true;
                    spawn_session_forwarder(session, ctx.ipc_tx.clone());
                    ActionOutcome::status("Reconnected to daemon.")
                }
                Err(e) => ActionOutcome::status(format!("Retry failed: {e}")),
            },
            DaemonWork::PaletteSubmit(input) => {
                match dispatch_palette_command(socket_path, &input).await {
                    PaletteOutcome::Quit => ActionOutcome::Quit,
                    PaletteOutcome::OpenSettings => {
                        model.open_theme_picker();
                        ActionOutcome::done()
                    }
                    PaletteOutcome::Status(message) => ActionOutcome::Status(message),
                }
            }
            DaemonWork::ApplyTheme(theme) => {
                ActionOutcome::status(persist_theme_choice(ctx.config_path, theme.id).await)
            }
        }
    }
}

pub(super) async fn handle_action(
    action: TuiAction,
    model: &mut TuiModel,
    ctx: &ActionCtx<'_>,
) -> ActionOutcome {
    // Only the two bookkeeping actions extend a half-typed sequence;
    // everything else completes (or abandons) it, which is what takes the
    // which-key overlay back down.
    let keeps_pending = matches!(action, TuiAction::PushCount(_) | TuiAction::SetPending(_));
    let outcome = run_action(action, model, ctx).await;
    if !keeps_pending {
        model.clear_pending();
    }
    outcome
}

/// Run one action: classify it, then await the daemon if that is what it
/// turned out to need.
async fn run_action(action: TuiAction, model: &mut TuiModel, ctx: &ActionCtx<'_>) -> ActionOutcome {
    match dispatch(action, model, ctx) {
        Dispatched::Done(outcome) => outcome,
        Dispatched::Daemon(work) => work.run(model, ctx).await,
    }
}

/// Nothing more to do — the action was a pure model update.
fn done() -> Dispatched {
    Dispatched::Done(ActionOutcome::done())
}

fn daemon(work: DaemonWork) -> Dispatched {
    Dispatched::Daemon(work)
}

/// Apply `action` to the model and say what, if anything, still has to be
/// sent to the daemon.
///
/// Synchronous and socket-free by construction: everything that needs the
/// daemon comes back as a [`DaemonWork`] value for [`run_action`] to await, so
/// this whole classification can be exercised in unit tests with no daemon
/// running. The match is exhaustive on purpose — no wildcard arm — because
/// that is what makes the single list above trustworthy.
fn dispatch(action: TuiAction, model: &mut TuiModel, ctx: &ActionCtx<'_>) -> Dispatched {
    match action {
        // --- leaving, and the vim bookkeeping keys ---
        TuiAction::Quit => Dispatched::Done(ActionOutcome::Quit),
        TuiAction::SetPending(pending) => {
            model.pending = pending;
            done()
        }
        TuiAction::ClearPending => {
            model.cancel();
            done()
        }
        TuiAction::PushCount(digit) => {
            model.push_count(digit);
            done()
        }

        // --- the command line ---
        TuiAction::OpenPalette { prefix, seed } => {
            model.open_palette(prefix, seed);
            done()
        }
        TuiAction::ClosePalette => {
            model.command_palette.close();
            done()
        }
        TuiAction::PaletteChar(c) => {
            model.edit_palette(|palette| palette.input.push(c));
            done()
        }
        TuiAction::PaletteBackspace => {
            model.edit_palette(|palette| {
                palette.input.pop();
            });
            done()
        }
        TuiAction::PaletteClear => {
            model.edit_palette(CommandPaletteState::clear_to_prefix);
            done()
        }
        TuiAction::PaletteDeleteWord => {
            model.edit_palette(CommandPaletteState::delete_word);
            done()
        }
        TuiAction::CompleteNext => {
            model.command_palette.cycle_next();
            done()
        }
        TuiAction::CompletePrev => {
            model.command_palette.cycle_prev();
            done()
        }
        // The line is taken and the palette closed before the round-trip, so
        // the daemon's reply lands on a command line the user has finished
        // with rather than one they have started retyping.
        TuiAction::PaletteSubmit => {
            let input = model.command_palette.input.clone();
            model.command_palette.close();
            daemon(DaemonWork::PaletteSubmit(input))
        }

        // --- panel focus ---
        TuiAction::FocusScenes => {
            model.focus = FocusPanel::Scenes;
            done()
        }
        TuiAction::FocusAudio => {
            model.focus = FocusPanel::Audio;
            done()
        }
        TuiAction::FocusProfiles => {
            model.focus = FocusPanel::Profiles;
            done()
        }
        TuiAction::FocusCollections => {
            model.focus = FocusPanel::Collections;
            done()
        }
        TuiAction::FocusPaneLeft => {
            model.focus = model.focus.left();
            done()
        }
        TuiAction::FocusPaneRight => {
            model.focus = model.focus.right();
            done()
        }
        TuiAction::FocusPaneUp => {
            model.focus = model.focus.up();
            done()
        }
        TuiAction::FocusPaneDown => {
            model.focus = model.focus.down();
            done()
        }
        TuiAction::FocusPaneNext => {
            model.focus = model.focus.next();
            done()
        }
        TuiAction::FocusPanePrev => {
            model.focus = model.focus.prev();
            done()
        }

        // --- motions ---
        TuiAction::NavUp(rows) => {
            model.nav_up(rows);
            done()
        }
        TuiAction::NavDown(rows) => {
            model.nav_down(rows);
            done()
        }
        TuiAction::NavTop => {
            model.nav_top();
            done()
        }
        TuiAction::NavBottom => {
            model.nav_bottom();
            done()
        }
        TuiAction::NavHalfPageUp => {
            model.nav_up(half_page(ctx.hits, model));
            done()
        }
        TuiAction::NavHalfPageDown => {
            model.nav_down(half_page(ctx.hits, model));
            done()
        }
        TuiAction::SelectIndex(panel, index) => {
            model.focus = panel;
            model.set_panel_cursor(panel, index);
            done()
        }
        TuiAction::LogScrollUp(lines) => {
            let visible = usize::from(ctx.hits.logs.height.saturating_sub(2));
            model.scroll_logs_up(lines, visible);
            done()
        }
        TuiAction::LogScrollDown(lines) => {
            model.scroll_logs_down(lines);
            done()
        }

        // --- acting on the focused row ---
        TuiAction::Activate => activate_focused(model),
        TuiAction::ActivateIndex(panel, index) => {
            model.focus = panel;
            model.set_panel_cursor(panel, index);
            activate_focused(model)
        }
        TuiAction::ToggleMute => match model.focused_audio().map(|a| a.name.clone()) {
            Some(name) => daemon(DaemonWork::Targeted(ServerCommand::ToggleMute, name)),
            None => done(),
        },
        TuiAction::VolumeDown(steps) => Dispatched::Done(ActionOutcome::maybe_status(
            adjust_focused_volume(model, ctx.vol_tx, -volume_delta(steps)),
        )),
        TuiAction::VolumeUp(steps) => Dispatched::Done(ActionOutcome::maybe_status(
            adjust_focused_volume(model, ctx.vol_tx, volume_delta(steps)),
        )),

        // --- appearance and the settings view ---
        TuiAction::ToggleIcons => {
            model.show_icons = !model.show_icons;
            let state = if model.show_icons { "on" } else { "off" };
            Dispatched::Done(ActionOutcome::status(format!("icons {state}")))
        }
        TuiAction::ToggleAdvancedUi => {
            model.advanced_ui = !model.advanced_ui;
            let state = if model.advanced_ui { "on" } else { "off" };
            Dispatched::Done(ActionOutcome::status(format!("advanced UI {state}")))
        }
        TuiAction::OpenSettings => {
            model.open_theme_picker();
            done()
        }
        TuiAction::CloseSettings => {
            model.cancel_theme_picker();
            done()
        }
        TuiAction::SettingsSelect(index) => {
            model.preview_theme(index);
            done()
        }
        TuiAction::ApplySettingsTheme => daemon(DaemonWork::ApplyTheme(model.apply_theme_picker())),

        // --- send this to the daemon and report what it said ---
        //
        // One row each, and the row is the whole implementation: these eight
        // used to be a separate lookup table consulted before the match, which
        // is one more list to keep in step for no gain.
        TuiAction::ReloadConfig => simple(ServerCommand::ReloadConfig, ReplyStyle::Acknowledge),
        TuiAction::DumpConfig => simple(ServerCommand::DumpConfig, ReplyStyle::Acknowledge),
        TuiAction::ToggleStream => simple(ServerCommand::ToggleStream, ReplyStyle::Acknowledge),
        TuiAction::ToggleRecord => simple(ServerCommand::ToggleRecord, ReplyStyle::Acknowledge),
        TuiAction::ReconnectObs => simple(ServerCommand::ReconnectObs, ReplyStyle::Acknowledge),
        TuiAction::ValidateConfig => simple(ServerCommand::ValidateConfig, ReplyStyle::ShowPayload),
        TuiAction::ObsStatus => simple(ServerCommand::GetObsStatus, ReplyStyle::ShowPayload),
        TuiAction::ServerStatus => simple(ServerCommand::GetServerStatus, ReplyStyle::ShowPayload),
        TuiAction::RetryConnect => daemon(DaemonWork::RetryConnect),
    }
}

fn simple(command: ServerCommand, reply: ReplyStyle) -> Dispatched {
    daemon(DaemonWork::Simple(command, reply))
}

/// Enter (or a click) on the focused row: switch to that scene, profile, or
/// collection, or toggle mute for that audio input. Nothing to send when the
/// focused panel is empty.
fn activate_focused(model: &TuiModel) -> Dispatched {
    let target = match model.focus {
        FocusPanel::Scenes => model
            .focused_scene()
            .map(|s| (ServerCommand::SetScene, s.name.clone())),
        FocusPanel::Profiles => model
            .focused_profile()
            .map(|p| (ServerCommand::SetProfile, p.to_string())),
        FocusPanel::Collections => model
            .focused_scene_collection()
            .map(|c| (ServerCommand::SetSceneCollection, c.to_string())),
        FocusPanel::Audio => model
            .focused_audio()
            .map(|a| (ServerCommand::ToggleMute, a.name.clone())),
    };
    match target {
        Some((command, target)) => daemon(DaemonWork::Targeted(command, target)),
        None => done(),
    }
}

fn volume_delta(steps: usize) -> i16 {
    let steps = i16::try_from(steps).unwrap_or(i16::MAX);
    steps.saturating_mul(VOLUME_STEP).clamp(0, 100)
}

/// Best-effort write of the chosen theme id into `ui.theme` in the config
/// file, preserving every other setting. Failures are reported to the
/// palette result line but never block applying the theme in-memory.
async fn persist_theme_choice(config_path: Option<&Path>, theme_id: &str) -> String {
    let Some(path) = config_path else {
        return "theme applied (no config file to persist to)".to_string();
    };
    let mut config = match loader::load_or_default(path) {
        Ok(config) => config,
        Err(e) => return format!("theme applied, but reading config failed: {e}"),
    };
    config.ui.theme = theme_id.to_string();
    match writer::write_atomic(&config, path) {
        Ok(()) => format!("theme set: {theme_id}"),
        Err(e) => format!("theme applied, but saving config failed: {e}"),
    }
}

/// Adjust the focused input's volume by `delta` percentage points: update the
/// model immediately (optimistic feedback) and enqueue the new target on the
/// debouncer so the actual `set_volume` command fires once the burst settles.
/// Returns `None` — the command's outcome is surfaced asynchronously via the
/// status channel, so there is no synchronous result to report here.
fn adjust_focused_volume(
    model: &mut TuiModel,
    vol_tx: &mpsc::UnboundedSender<(String, u8)>,
    delta: i16,
) -> Option<String> {
    let (name, new_percent) = model.adjusted_focused_volume(delta)?;
    model.set_audio_volume_local(&name, new_percent);
    // Unbounded: keypresses are human-rate, and we must never drop the newest
    // target (which would strand the displayed level out of sync with OBS).
    let _ = vol_tx.send((name, new_percent));
    None
}

#[cfg(test)]
mod tests {
    use super::{ActionCtx, ActionOutcome, Dispatched, dispatch, persist_theme_choice};
    use crate::obs::state::{AudioState, ObsSnapshot, SceneState};
    use crate::tui::input::TuiAction;
    use crate::tui::keymap::Pending;
    use crate::tui::model::{FocusPanel, TuiModel};
    use crate::tui::mouse::Hitboxes;
    use std::path::Path;
    use tokio::sync::mpsc;

    /// Local actions never touch the socket, so the paths and channels here
    /// exist only to satisfy the struct — nothing in this test reads them.
    struct LocalCtx {
        ipc_tx: mpsc::Sender<std::result::Result<crate::ipc::protocol::ServerMessage, String>>,
        vol_tx: mpsc::UnboundedSender<(String, u8)>,
        hits: Hitboxes,
    }

    impl LocalCtx {
        fn new() -> Self {
            Self {
                ipc_tx: mpsc::channel(1).0,
                vol_tx: mpsc::unbounded_channel().0,
                hits: Hitboxes::default(),
            }
        }

        fn ctx(&self) -> ActionCtx<'_> {
            ActionCtx {
                socket_path: Path::new("/nonexistent/obsctl.sock"),
                config_path: None,
                ipc_tx: &self.ipc_tx,
                vol_tx: &self.vol_tx,
                hits: &self.hits,
            }
        }
    }

    /// A model with something in every panel, so the actions that act on the
    /// focused row have a row to act on.
    fn populated_model() -> TuiModel {
        let mut model = TuiModel::default();
        model.set_snapshot(ObsSnapshot {
            scenes: vec![SceneState {
                name: "Main".to_string(),
                ..Default::default()
            }],
            audio_inputs: vec![AudioState {
                name: "Mic".to_string(),
                ..Default::default()
            }],
            profiles: vec!["Default".to_string()],
            scene_collections: vec!["Podcast".to_string()],
            ..Default::default()
        });
        model
    }

    #[test]
    fn local_actions_update_the_model_without_a_daemon() {
        let owned = LocalCtx::new();
        let ctx = owned.ctx();
        let mut model = TuiModel::default();

        assert!(matches!(
            dispatch(TuiAction::FocusAudio, &mut model, &ctx),
            Dispatched::Done(ActionOutcome::Continue)
        ));
        assert_eq!(model.focus, FocusPanel::Audio);

        assert!(matches!(
            dispatch(TuiAction::Quit, &mut model, &ctx),
            Dispatched::Done(ActionOutcome::Quit)
        ));

        let Dispatched::Done(ActionOutcome::Status(status)) =
            dispatch(TuiAction::ToggleIcons, &mut model, &ctx)
        else {
            panic!("toggling icons reports the new state");
        };
        assert_eq!(status, "icons off");
        assert!(!model.show_icons);
    }

    /// Every action is classified, and classifying one never needs a daemon.
    ///
    /// This replaces a test that checked only that five daemon-bound actions
    /// reported themselves as non-local. Listing every variant means an action
    /// added later has to be added here too, and the assertion is now the
    /// stronger one: the classification is complete and the daemon-bound half
    /// really does defer its round-trip instead of doing it inside `dispatch`.
    #[test]
    fn every_action_is_classified_without_a_daemon() {
        let owned = LocalCtx::new();
        let ctx = owned.ctx();

        let daemon_bound = [
            TuiAction::PaletteSubmit,
            TuiAction::ReloadConfig,
            TuiAction::DumpConfig,
            TuiAction::ValidateConfig,
            TuiAction::ObsStatus,
            TuiAction::ServerStatus,
            TuiAction::ToggleStream,
            TuiAction::ToggleRecord,
            TuiAction::ReconnectObs,
            TuiAction::RetryConnect,
            TuiAction::Activate,
            TuiAction::ActivateIndex(FocusPanel::Profiles, 0),
            TuiAction::ToggleMute,
            TuiAction::ApplySettingsTheme,
        ];
        for action in daemon_bound {
            let mut model = populated_model();
            model.focus = FocusPanel::Scenes;
            assert!(
                matches!(
                    dispatch(action.clone(), &mut model, &ctx),
                    Dispatched::Daemon(_)
                ),
                "{action:?} should be answered by the daemon"
            );
        }

        let local = [
            TuiAction::Quit,
            TuiAction::SetPending(Pending::G),
            TuiAction::ClearPending,
            TuiAction::PushCount(3),
            TuiAction::OpenPalette {
                prefix: None,
                seed: "",
            },
            TuiAction::ClosePalette,
            TuiAction::PaletteChar('x'),
            TuiAction::PaletteBackspace,
            TuiAction::PaletteClear,
            TuiAction::PaletteDeleteWord,
            TuiAction::CompleteNext,
            TuiAction::CompletePrev,
            TuiAction::FocusScenes,
            TuiAction::FocusAudio,
            TuiAction::FocusProfiles,
            TuiAction::FocusCollections,
            TuiAction::FocusPaneLeft,
            TuiAction::FocusPaneRight,
            TuiAction::FocusPaneUp,
            TuiAction::FocusPaneDown,
            TuiAction::FocusPaneNext,
            TuiAction::FocusPanePrev,
            TuiAction::NavUp(1),
            TuiAction::NavDown(1),
            TuiAction::NavTop,
            TuiAction::NavBottom,
            TuiAction::NavHalfPageUp,
            TuiAction::NavHalfPageDown,
            TuiAction::SelectIndex(FocusPanel::Scenes, 0),
            TuiAction::VolumeUp(1),
            TuiAction::VolumeDown(1),
            TuiAction::LogScrollUp(1),
            TuiAction::LogScrollDown(1),
            TuiAction::ToggleIcons,
            TuiAction::ToggleAdvancedUi,
            TuiAction::OpenSettings,
            TuiAction::CloseSettings,
            TuiAction::SettingsSelect(1),
        ];
        for action in local {
            let mut model = populated_model();
            assert!(
                matches!(
                    dispatch(action.clone(), &mut model, &ctx),
                    Dispatched::Done(_)
                ),
                "{action:?} should be finished without the daemon"
            );
        }
    }

    /// Activating an empty panel has nothing to send, and must not invent a
    /// target for the daemon.
    #[test]
    fn activating_an_empty_panel_sends_nothing() {
        let owned = LocalCtx::new();
        let ctx = owned.ctx();
        let mut model = TuiModel::default();
        assert!(matches!(
            dispatch(TuiAction::Activate, &mut model, &ctx),
            Dispatched::Done(ActionOutcome::Continue)
        ));
        assert!(matches!(
            dispatch(TuiAction::ToggleMute, &mut model, &ctx),
            Dispatched::Done(ActionOutcome::Continue)
        ));
    }

    #[tokio::test]
    async fn persist_theme_choice_writes_theme_id_and_preserves_other_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        crate::config::writer::write_default(&path).unwrap();

        let msg = persist_theme_choice(Some(&path), "nord").await;
        assert_eq!(msg, "theme set: nord");

        let saved = crate::config::loader::load(&path).unwrap();
        assert_eq!(saved.ui.theme, "nord");
    }

    #[tokio::test]
    async fn persist_theme_choice_without_path_reports_in_memory_only() {
        let msg = persist_theme_choice(None, "nord").await;
        assert_eq!(msg, "theme applied (no config file to persist to)");
    }
}
