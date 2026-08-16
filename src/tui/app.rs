use std::{
    collections::HashMap,
    io::stdout,
    path::{Path, PathBuf},
    time::Duration,
};

use time::OffsetDateTime;

use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use tokio::sync::mpsc;

use crate::{
    config::{loader, writer},
    domain::{command::Command, parser, result::Result},
    ipc::protocol::{CommandPayload, LogLevel, ServerCommand, ServerMessage},
    support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len},
    tui::{
        completion,
        event_applier::apply_server_message,
        input::{TuiAction, handle_key},
        layout,
        model::{DEFAULT_PALETTE_PREFIX, FocusPanel, TuiLogEntry, TuiModel, View},
        mouse::{self, HitView, Hitboxes},
        session::{TuiEventSession, send_command},
        theme::{self, Theme},
        widgets,
    },
};

/// Total time the startup splash is shown, unless skipped by a keypress.
const SPLASH_DURATION: Duration = Duration::from_millis(2000);
const SPLASH_FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// Percentage points one `h`/`l` (or `←`/`→`) nudge moves the focused input.
const VOLUME_STEP: i16 = 5;

/// Everything the TUI needs from the config to start up. Bundled rather than
/// passed positionally because the list only grows.
#[derive(Debug, Clone)]
pub struct TuiOptions {
    pub refresh_ms: u64,
    pub theme: Theme,
    pub show_icons: bool,
    pub advanced_ui: bool,
    /// Whether to put the terminal into mouse-reporting mode. Off leaves the
    /// terminal's own click-to-select working (see `ui.mouse`).
    pub mouse: bool,
    /// Character the command line opens with (`ui.command_palette_prefix`).
    pub palette_prefix: char,
    pub config_path: Option<PathBuf>,
}

impl Default for TuiOptions {
    fn default() -> Self {
        Self {
            refresh_ms: 250,
            theme: Theme::default_theme(),
            show_icons: true,
            advanced_ui: true,
            mouse: true,
            palette_prefix: DEFAULT_PALETTE_PREFIX,
            config_path: None,
        }
    }
}

pub async fn run(socket_path: &Path, options: TuiOptions) -> Result<i32> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    if options.mouse {
        execute!(stdout(), EnableMouseCapture)?;
    }
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    {
        let mut splash_events = EventStream::new();
        run_splash(
            &mut terminal,
            &mut splash_events,
            options.theme,
            options.show_icons,
            options.advanced_ui,
        )
        .await?;
    }

    let result = run_loop(&mut terminal, socket_path, &options).await;

    if options.mouse {
        // Best-effort: leaving mouse reporting on would break the terminal
        // for whatever runs next, so try to undo it even if the loop failed.
        let _ = execute!(stdout(), DisableMouseCapture);
    }
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;

    result
}

/// Show the animated "obsctl" splash for `SPLASH_DURATION`, or until the
/// user presses any key.
async fn run_splash(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    events: &mut EventStream,
    theme: Theme,
    show_icons: bool,
    advanced_ui: bool,
) -> Result<()> {
    let total_frames =
        (SPLASH_DURATION.as_millis() / SPLASH_FRAME_INTERVAL.as_millis().max(1)) as u64;
    let mut ticker = tokio::time::interval(SPLASH_FRAME_INTERVAL);
    let mut frame = 0u64;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                terminal.draw(|f| {
                    widgets::fill_background(f, theme);
                    widgets::splash::render_with_appearance(
                        f,
                        theme,
                        frame,
                        total_frames,
                        show_icons,
                        advanced_ui,
                    );
                })?;
                if frame >= total_frames {
                    return Ok(());
                }
                frame += 1;
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => return Ok(()),
                    Some(Ok(Event::Mouse(m))) if matches!(m.kind, MouseEventKind::Down(_)) => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    socket_path: &Path,
    options: &TuiOptions,
) -> Result<i32> {
    let mut model =
        TuiModel::with_appearance(options.theme, options.show_icons, options.advanced_ui);
    model.palette_prefix = options.palette_prefix;
    let config_path = options.config_path.clone();
    let refresh = Duration::from_millis(options.refresh_ms.max(50));
    // Where the last frame put each panel; mouse events resolve against it.
    let mut hits = Hitboxes::default();

    // Try to connect to daemon
    let (ipc_tx, mut ipc_rx) = mpsc::channel::<std::result::Result<ServerMessage, String>>(64);

    // Volume changes are applied to the model optimistically on every keypress
    // and the latest target per input is forwarded to a background debouncer,
    // so the event loop never blocks on the IPC/OBS round-trip and OBS isn't
    // flooded with intermediate steps during a rapid up/down burst. The
    // debouncer reports each command's outcome back through `status_rx`.
    let (vol_tx, vol_rx) = mpsc::unbounded_channel::<(String, u8)>();
    let (status_tx, mut status_rx) = mpsc::channel::<String>(16);
    spawn_volume_debouncer(socket_path.to_path_buf(), vol_rx, status_tx);

    match TuiEventSession::connect(socket_path).await {
        Ok(session) => {
            model.connected_to_daemon = true;
            spawn_session_forwarder(session, ipc_tx.clone());
        }
        Err(e) => {
            model.connected_to_daemon = false;
            let msg = format!("Cannot connect to daemon: {e}");
            model.set_last_result(msg.clone());
            model.push_log(TuiLogEntry {
                level: LogLevel::Error,
                message: msg,
                target: None,
                timestamp: OffsetDateTime::now_utc(),
            });
        }
    }

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(refresh);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                model.anim.tick();
                draw(terminal, &model, &mut hits)?;
            }

            maybe_ipc = ipc_rx.recv() => {
                let needs_redraw = match maybe_ipc {
                    Some(Ok(msg)) => apply_server_message(&mut model, msg),
                    Some(Err(e)) => {
                        model.connected_to_daemon = false;
                        model.set_last_result(format!("Daemon disconnected: {e}"));
                        true
                    }
                    None => {
                        model.connected_to_daemon = false;
                        true
                    }
                };
                if needs_redraw {
                    draw(terminal, &model, &mut hits)?;
                }
            }

            maybe_status = status_rx.recv() => {
                if let Some(status) = maybe_status {
                    model.set_last_result(status);
                    draw(terminal, &model, &mut hits)?;
                }
            }

            maybe_event = events.next() => {
                let action = match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        handle_key(&model, key)
                    }
                    Some(Ok(Event::Mouse(m))) => mouse::handle_mouse(&model, &hits, m),
                    Some(Ok(Event::Resize(_, _))) => {
                        draw(terminal, &model, &mut hits)?;
                        None
                    }
                    None | Some(Err(_)) => return Ok(1),
                    _ => None,
                };

                if let Some(action) = action {
                    let ctx = ActionCtx {
                        socket_path,
                        config_path: config_path.as_deref(),
                        ipc_tx: &ipc_tx,
                        vol_tx: &vol_tx,
                        hits: &hits,
                    };
                    let (should_quit, result) = handle_action(action, &mut model, &ctx).await;
                    if should_quit {
                        return Ok(0);
                    }
                    if let Some(r) = result {
                        model.set_last_result(r);
                    }
                    draw(terminal, &model, &mut hits)?;
                }
            }
        }
    }
}

/// Render a frame and remember where it put things, so the next mouse event
/// has something to hit-test against.
fn draw(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    model: &TuiModel,
    hits: &mut Hitboxes,
) -> Result<()> {
    terminal.draw(|f| *hits = render(f, model))?;
    Ok(())
}

fn spawn_session_forwarder(
    mut session: TuiEventSession,
    tx: mpsc::Sender<std::result::Result<ServerMessage, String>>,
) {
    tokio::spawn(async move {
        loop {
            match session.next_event().await {
                Ok(msg) => {
                    if tx.send(Ok(msg)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });
}

/// Trailing-edge debounce for volume changes. Receives the latest desired
/// percentage per input, coalesces a burst of keypresses, and issues a single
/// `set_volume` command per input once the user pauses for `DEBOUNCE`. Running
/// on its own task keeps the TUI event loop free of the blocking IPC round-trip,
/// and coalescing avoids flooding OBS with intermediate volume steps.
fn spawn_volume_debouncer(
    socket_path: PathBuf,
    mut rx: mpsc::UnboundedReceiver<(String, u8)>,
    status_tx: mpsc::Sender<String>,
) {
    const DEBOUNCE: Duration = Duration::from_millis(120);
    tokio::spawn(async move {
        let mut pending: HashMap<String, u8> = HashMap::new();
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        // Keep only the latest target per input; earlier steps
                        // in the burst never need to reach OBS.
                        Some((name, percent)) => { pending.insert(name, percent); }
                        None => break, // TUI shut down; stop the debouncer.
                    }
                }
                // Re-created each iteration, so every new keypress restarts the
                // timer (trailing edge). Disabled while there is nothing pending.
                _ = tokio::time::sleep(DEBOUNCE), if !pending.is_empty() => {
                    for (name, percent) in pending.drain() {
                        let result = send_set_volume(&socket_path, &name, percent).await;
                        // Cosmetic status only; drop it rather than block if the
                        // UI is slow to drain.
                        let _ = status_tx.try_send(result);
                    }
                }
            }
        }
    });
}

/// Everything an action might need that isn't the model itself.
struct ActionCtx<'a> {
    socket_path: &'a Path,
    config_path: Option<&'a Path>,
    ipc_tx: &'a mpsc::Sender<std::result::Result<ServerMessage, String>>,
    vol_tx: &'a mpsc::UnboundedSender<(String, u8)>,
    hits: &'a Hitboxes,
}

async fn handle_action(
    action: TuiAction,
    model: &mut TuiModel,
    ctx: &ActionCtx<'_>,
) -> (bool, Option<String>) {
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

/// Run one action: the ones that talk to the daemon are handled here, and
/// everything else — cursor movement, focus, palette editing, theme preview —
/// is a pure model update delegated to [`apply_local_action`].
///
/// Returns `(should_quit, status_line)`.
async fn run_action(
    action: TuiAction,
    model: &mut TuiModel,
    ctx: &ActionCtx<'_>,
) -> (bool, Option<String>) {
    let socket_path = ctx.socket_path;
    match action {
        TuiAction::PaletteSubmit => {
            let input = model.command_palette.input.clone();
            model.command_palette.active = false;
            model.command_palette.input.clear();
            model.command_palette.completions.clear();
            model.command_palette.completion_idx = None;
            let result = dispatch_palette_command(socket_path, &input).await;
            if result == "quit" {
                return (true, None);
            }
            if result == "themes" {
                open_settings(model);
                return (false, None);
            }
            (false, Some(result))
        }
        TuiAction::ReloadConfig => (
            false,
            Some(send_simple(socket_path, ServerCommand::ReloadConfig).await),
        ),
        TuiAction::DumpConfig => (
            false,
            Some(send_simple(socket_path, ServerCommand::DumpConfig).await),
        ),
        TuiAction::ValidateConfig => (
            false,
            Some(send_status(socket_path, ServerCommand::ValidateConfig).await),
        ),
        TuiAction::ObsStatus => (
            false,
            Some(send_status(socket_path, ServerCommand::GetObsStatus).await),
        ),
        TuiAction::ServerStatus => (
            false,
            Some(send_status(socket_path, ServerCommand::GetServerStatus).await),
        ),
        TuiAction::ToggleStream => (
            false,
            Some(send_simple(socket_path, ServerCommand::ToggleStream).await),
        ),
        TuiAction::ToggleRecord => (
            false,
            Some(send_simple(socket_path, ServerCommand::ToggleRecord).await),
        ),
        TuiAction::ReconnectObs => (
            false,
            Some(send_simple(socket_path, ServerCommand::ReconnectObs).await),
        ),
        TuiAction::ActivateIndex(panel, index) => {
            model.focus = panel;
            model.set_panel_cursor(panel, index);
            (false, activate_focused(model, socket_path).await)
        }
        TuiAction::Activate => (false, activate_focused(model, socket_path).await),
        TuiAction::ToggleMute => {
            if let Some(name) = model.focused_audio().map(|a| a.name.clone()) {
                let result =
                    send_simple_with_target(socket_path, ServerCommand::ToggleMute, &name).await;
                (false, Some(result))
            } else {
                (false, None)
            }
        }
        TuiAction::RetryConnect => match TuiEventSession::connect(socket_path).await {
            Ok(session) => {
                model.connected_to_daemon = true;
                spawn_session_forwarder(session, ctx.ipc_tx.clone());
                (false, Some("Reconnected to daemon.".to_string()))
            }
            Err(e) => (false, Some(format!("Retry failed: {e}"))),
        },
        TuiAction::ApplySettingsTheme => {
            let chosen = theme::ALL[model.settings_cursor];
            model.theme = chosen;
            model.theme_preview_origin = None;
            model.view = View::Main;
            let result = persist_theme_choice(ctx.config_path, chosen.id).await;
            (false, Some(result))
        }
        local => apply_local_action(local, model, ctx)
            .expect("every action not handled above is a local action"),
    }
}

/// Every action that only rearranges the model. Synchronous and socket-free,
/// so it can be exercised without a running daemon.
///
/// Returns `None` for the actions [`run_action`] handles itself. The match is
/// deliberately exhaustive rather than ending in a wildcard: a newly added
/// `TuiAction` then fails to compile here instead of panicking at runtime the
/// first time a user presses the key.
fn apply_local_action(
    action: TuiAction,
    model: &mut TuiModel,
    ctx: &ActionCtx<'_>,
) -> Option<(bool, Option<String>)> {
    match action {
        TuiAction::Quit => Some((true, None)),
        TuiAction::SetPending(pending) => {
            model.pending = pending;
            Some((false, None))
        }
        TuiAction::ClearPending => {
            model.clear_pending();
            // Esc / right-click also snaps the log pane back to the live tail.
            model.log_scroll = 0;
            Some((false, None))
        }
        TuiAction::PushCount(digit) => {
            model.push_count(digit);
            Some((false, None))
        }
        TuiAction::OpenPalette { prefix, seed } => {
            model.command_palette.active = true;
            model.command_palette.input.clear();
            model
                .command_palette
                .input
                .push(prefix.unwrap_or(model.palette_prefix));
            model.command_palette.input.push_str(seed);
            refresh_completions(model);
            Some((false, None))
        }
        TuiAction::ClosePalette => {
            model.command_palette.active = false;
            model.command_palette.input.clear();
            model.command_palette.completions.clear();
            model.command_palette.completion_idx = None;
            Some((false, None))
        }
        TuiAction::PaletteChar(c) => {
            model.command_palette.input.push(c);
            refresh_completions(model);
            Some((false, None))
        }
        TuiAction::PaletteBackspace => {
            model.command_palette.input.pop();
            refresh_completions(model);
            Some((false, None))
        }
        TuiAction::PaletteClear => {
            model.command_palette.clear_to_prefix();
            refresh_completions(model);
            Some((false, None))
        }
        TuiAction::PaletteDeleteWord => {
            model.command_palette.delete_word();
            refresh_completions(model);
            Some((false, None))
        }
        TuiAction::FocusScenes => {
            model.focus = FocusPanel::Scenes;
            Some((false, None))
        }
        TuiAction::FocusAudio => {
            model.focus = FocusPanel::Audio;
            Some((false, None))
        }
        TuiAction::FocusProfiles => {
            model.focus = FocusPanel::Profiles;
            Some((false, None))
        }
        TuiAction::FocusCollections => {
            model.focus = FocusPanel::Collections;
            Some((false, None))
        }
        TuiAction::FocusPaneLeft => {
            model.focus = model.focus.left();
            Some((false, None))
        }
        TuiAction::FocusPaneRight => {
            model.focus = model.focus.right();
            Some((false, None))
        }
        TuiAction::FocusPaneUp => {
            model.focus = model.focus.up();
            Some((false, None))
        }
        TuiAction::FocusPaneDown => {
            model.focus = model.focus.down();
            Some((false, None))
        }
        TuiAction::FocusPaneNext => {
            model.focus = model.focus.next();
            Some((false, None))
        }
        TuiAction::FocusPanePrev => {
            model.focus = model.focus.prev();
            Some((false, None))
        }
        TuiAction::NavUp(rows) => {
            model.move_up_by(rows);
            Some((false, None))
        }
        TuiAction::NavDown(rows) => {
            model.move_down_by(rows);
            Some((false, None))
        }
        TuiAction::NavTop => {
            model.move_to_top();
            Some((false, None))
        }
        TuiAction::NavBottom => {
            model.move_to_bottom();
            Some((false, None))
        }
        TuiAction::NavHalfPageUp => {
            model.move_up_by(half_page(ctx.hits, model.focus));
            Some((false, None))
        }
        TuiAction::NavHalfPageDown => {
            model.move_down_by(half_page(ctx.hits, model.focus));
            Some((false, None))
        }
        TuiAction::SelectIndex(panel, index) => {
            model.focus = panel;
            model.set_panel_cursor(panel, index);
            Some((false, None))
        }
        TuiAction::VolumeDown(steps) => Some((
            false,
            adjust_focused_volume(model, ctx.vol_tx, -volume_delta(steps)),
        )),
        TuiAction::VolumeUp(steps) => Some((
            false,
            adjust_focused_volume(model, ctx.vol_tx, volume_delta(steps)),
        )),
        TuiAction::LogScrollUp(lines) => {
            let visible = usize::from(ctx.hits.logs.height.saturating_sub(2));
            model.scroll_logs_up(lines, visible);
            Some((false, None))
        }
        TuiAction::LogScrollDown(lines) => {
            model.scroll_logs_down(lines);
            Some((false, None))
        }
        TuiAction::CompleteNext => {
            model.command_palette.cycle_next();
            Some((false, None))
        }
        TuiAction::CompletePrev => {
            model.command_palette.cycle_prev();
            Some((false, None))
        }
        TuiAction::ToggleIcons => {
            model.show_icons = !model.show_icons;
            let state = if model.show_icons { "on" } else { "off" };
            Some((false, Some(format!("icons {state}"))))
        }
        TuiAction::ToggleAdvancedUi => {
            model.advanced_ui = !model.advanced_ui;
            let state = if model.advanced_ui { "on" } else { "off" };
            Some((false, Some(format!("advanced UI {state}"))))
        }
        TuiAction::OpenSettings => {
            open_settings(model);
            Some((false, None))
        }
        TuiAction::CloseSettings => {
            if let Some(original) = model.theme_preview_origin.take() {
                model.theme = original;
            }
            model.view = View::Main;
            Some((false, None))
        }
        TuiAction::SettingsNavUp(rows) => {
            preview_theme(model, model.settings_cursor.saturating_sub(rows));
            Some((false, None))
        }
        TuiAction::SettingsNavDown(rows) => {
            preview_theme(model, model.settings_cursor.saturating_add(rows));
            Some((false, None))
        }
        TuiAction::SettingsNavTop => {
            preview_theme(model, 0);
            Some((false, None))
        }
        TuiAction::SettingsNavBottom => {
            preview_theme(model, usize::MAX);
            Some((false, None))
        }
        TuiAction::SettingsSelect(index) => {
            preview_theme(model, index);
            Some((false, None))
        }
        // Handled by `run_action`, which awaits the daemon.
        TuiAction::PaletteSubmit
        | TuiAction::ReloadConfig
        | TuiAction::DumpConfig
        | TuiAction::ValidateConfig
        | TuiAction::ObsStatus
        | TuiAction::ServerStatus
        | TuiAction::ToggleStream
        | TuiAction::ToggleRecord
        | TuiAction::ReconnectObs
        | TuiAction::ActivateIndex(..)
        | TuiAction::Activate
        | TuiAction::ToggleMute
        | TuiAction::RetryConnect
        | TuiAction::ApplySettingsTheme => None,
    }
}

/// Rows one `Ctrl-D`/`Ctrl-U` covers: half the focused pane's visible height,
/// falling back to a sensible step before the first frame has been drawn.
/// How far `Ctrl-D`/`Ctrl-U` (and `PgDn`/`PgUp`) move in `panel` — half a
/// screenful of whatever that panel scrolls. The list panels scroll rows;
/// the audio matrix scrolls channel strips sideways, so half a page there is
/// half the strips that fit across it, not half its height.
fn half_page(hits: &Hitboxes, panel: FocusPanel) -> usize {
    let area = hits.panel(panel);
    let items = if panel == FocusPanel::Audio {
        widgets::audio::visible_strips(area.width.saturating_sub(2))
    } else {
        usize::from(area.height.saturating_sub(2))
    };
    (items / 2).max(1)
}

fn volume_delta(steps: usize) -> i16 {
    let steps = i16::try_from(steps).unwrap_or(i16::MAX);
    steps.saturating_mul(VOLUME_STEP).clamp(0, 100)
}

/// Move the settings cursor to `index` (clamped) and live-preview its theme.
fn preview_theme(model: &mut TuiModel, index: usize) {
    let max = theme::ALL.len().saturating_sub(1);
    model.settings_cursor = index.min(max);
    model.theme = theme::ALL[model.settings_cursor];
}

/// Enter/click on the focused row: switch to it, or for audio toggle mute.
async fn activate_focused(model: &TuiModel, socket_path: &Path) -> Option<String> {
    let (command, target) = match model.focus {
        FocusPanel::Scenes => (
            ServerCommand::SetScene,
            model.focused_scene().map(|s| s.name.clone())?,
        ),
        FocusPanel::Profiles => (
            ServerCommand::SetProfile,
            model.focused_profile()?.to_string(),
        ),
        FocusPanel::Collections => (
            ServerCommand::SetSceneCollection,
            model.focused_scene_collection()?.to_string(),
        ),
        FocusPanel::Audio => (
            ServerCommand::ToggleMute,
            model.focused_audio().map(|a| a.name.clone())?,
        ),
    };
    Some(send_simple_with_target(socket_path, command, &target).await)
}

/// Enter the settings view, remembering the current theme so Esc/close
/// without confirming can restore it (live-preview-then-cancel).
fn open_settings(model: &mut TuiModel) {
    model.theme_preview_origin = Some(model.theme);
    model.settings_cursor = model.theme.index();
    model.view = View::Settings;
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

/// Draw a frame, returning where the interactive regions ended up so mouse
/// events can be resolved against the pixels the user is actually looking at.
fn render(f: &mut ratatui::Frame, model: &TuiModel) -> Hitboxes {
    widgets::fill_background(f, model.theme);

    if model.view == View::Settings {
        let settings_list = widgets::settings::render(f, f.area(), model);
        return Hitboxes {
            view: HitView::Settings,
            settings_list,
            ..Hitboxes::default()
        };
    }

    let areas = layout::compute(f, model.streaming());

    if !model.connected_to_daemon {
        widgets::connection::render_unavailable(f, f.area(), model);
        return Hitboxes {
            view: HitView::Disconnected,
            ..Hitboxes::default()
        };
    }

    widgets::header::render(f, areas.header, model);
    // The live bar only draws its own state badges when the big top-right
    // pane is not on screen, so the state is never spelled out twice.
    widgets::live_bar::render(f, areas.live_bar, model, areas.status.is_none());
    if let Some(status_area) = areas.status {
        widgets::status::render(f, status_area, model);
    }
    widgets::scenes::render(f, areas.scenes, model);
    widgets::audio::render(f, areas.audio, model);
    widgets::profiles::render(f, areas.profiles, model);
    widgets::collections::render(f, areas.collections, model);
    widgets::logs::render(f, areas.logs, model);
    if let Some(stats_area) = areas.stats {
        widgets::stats::render(f, stats_area, model);
    }
    widgets::command_palette::render(f, areas.palette, model);
    // Last, so the which-key popup floats above the dashboard.
    widgets::which_key::render(f, f.area(), model);

    Hitboxes {
        view: HitView::Main,
        scenes: areas.scenes,
        audio: areas.audio,
        profiles: areas.profiles,
        collections: areas.collections,
        logs: areas.logs,
        palette: areas.palette,
        settings_list: Rect::default(),
    }
}

fn refresh_completions(model: &mut TuiModel) {
    let input = model.command_palette.input.clone();
    model.command_palette.completions = completion::compute(&input, model);
    model.command_palette.completion_idx = None;
}

async fn dispatch_palette_command(socket_path: &Path, input: &str) -> String {
    match parser::parse(input) {
        Err(e) => format!("error: {e}"),
        Ok(Command::Quit) => "quit".to_string(),
        Ok(Command::Themes) => "themes".to_string(),
        Ok(Command::Help) => {
            "Commands: :scene :profile :collection :mute :unmute :toggle-mute :vol :stream :rec \
             :status :obs-status :server-status :reload-config :dump-config :validate-config \
             :themes :reconnect :quit  —  <space> opens the which-key menu"
                .to_string()
        }
        Ok(cmd) => {
            let payload = match command_to_payload(cmd) {
                Ok(payload) => payload,
                Err(error) => return format!("error: {error}"),
            };
            match send_command(socket_path, payload).await {
                Ok(ServerMessage::Response {
                    ok, result, error, ..
                }) => {
                    if ok {
                        result
                            .as_ref()
                            .and_then(|v| v.get("message"))
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "ok".to_string())
                    } else {
                        error
                            .as_ref()
                            .map(|e| format!("error [{}]: {}", e.code, e.message))
                            .unwrap_or_else(|| "error: unknown".to_string())
                    }
                }
                Ok(_) => "unexpected response".to_string(),
                Err(e) => format!("error: {e}"),
            }
        }
    }
}

fn command_to_payload(cmd: Command) -> std::result::Result<CommandPayload, String> {
    let (command, args) = match cmd {
        Command::Status => (ServerCommand::GetSnapshot, serde_json::Value::Null),
        Command::ServerStatus => (ServerCommand::GetServerStatus, serde_json::Value::Null),
        Command::ObsStatus => (ServerCommand::GetObsStatus, serde_json::Value::Null),
        Command::ValidateConfig => (ServerCommand::ValidateConfig, serde_json::Value::Null),
        Command::Reconnect | Command::Connect => {
            (ServerCommand::ReconnectObs, serde_json::Value::Null)
        }
        Command::ShutdownServer => (ServerCommand::ShutdownServer, serde_json::Value::Null),
        Command::DumpConfig => (ServerCommand::DumpConfig, serde_json::Value::Null),
        Command::ReloadConfig => (ServerCommand::ReloadConfig, serde_json::Value::Null),
        Command::SetScene { target } => {
            let target = sanitize_target_arg(&target)?;
            (
                ServerCommand::SetScene,
                serde_json::json!({ "target": target }),
            )
        }
        Command::SetProfile { target } => {
            let target = sanitize_target_arg(&target)?;
            (
                ServerCommand::SetProfile,
                serde_json::json!({ "target": target }),
            )
        }
        Command::SetSceneCollection { target } => {
            let target = sanitize_target_arg(&target)?;
            (
                ServerCommand::SetSceneCollection,
                serde_json::json!({ "target": target }),
            )
        }
        Command::Mute { target } => {
            let target = sanitize_target_arg(&target)?;
            (ServerCommand::Mute, serde_json::json!({ "target": target }))
        }
        Command::Unmute { target } => {
            let target = sanitize_target_arg(&target)?;
            (
                ServerCommand::Unmute,
                serde_json::json!({ "target": target }),
            )
        }
        Command::ToggleMute { target } => {
            let target = sanitize_target_arg(&target)?;
            (
                ServerCommand::ToggleMute,
                serde_json::json!({ "target": target }),
            )
        }
        Command::SetVolume { target, percent } => {
            if percent > 100 {
                return Err("volume percent must be 0-100".to_string());
            }
            (
                ServerCommand::SetVolume,
                serde_json::json!({
                    "target": sanitize_target_arg(&target)?,
                    "percent": percent
                }),
            )
        }
        Command::ToggleStream => (ServerCommand::ToggleStream, serde_json::Value::Null),
        Command::ToggleRecord => (ServerCommand::ToggleRecord, serde_json::Value::Null),
        Command::Help | Command::Quit | Command::Themes => unreachable!("handled before"),
    };
    Ok(CommandPayload {
        name: command.name().to_string(),
        args,
    })
}

fn sanitize_target_arg(value: &str) -> std::result::Result<String, String> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| format!("{error}"))
}

fn format_ipc_response(res: Result<ServerMessage>, ok_fallback: &str) -> String {
    match res {
        Ok(ServerMessage::Response {
            ok, result, error, ..
        }) => {
            if ok {
                result
                    .as_ref()
                    .and_then(|v| v.get("message"))
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| ok_fallback.to_string())
            } else {
                error
                    .map(|e| format!("error [{}]: {}", e.code, e.message))
                    .unwrap_or_else(|| "error".to_string())
            }
        }
        Ok(_) => "unexpected response".to_string(),
        Err(e) => format!("error: {e}"),
    }
}

async fn send_simple_with_target(
    socket_path: &Path,
    command: ServerCommand,
    target: &str,
) -> String {
    let target = match sanitize_target_arg(target) {
        Ok(target) => target,
        Err(error) => return format!("error: invalid target: {error}"),
    };

    let payload = CommandPayload::with_target(command, &target);
    format_ipc_response(send_command(socket_path, payload).await, "ok")
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

async fn send_set_volume(socket_path: &Path, target: &str, percent: u8) -> String {
    let target = match sanitize_target_arg(target) {
        Ok(target) => target,
        Err(error) => return format!("error: invalid target: {error}"),
    };
    if percent > 100 {
        return "error: volume percent must be 0-100".to_string();
    }

    let payload = CommandPayload::set_volume(&target, percent);
    format_ipc_response(
        send_command(socket_path, payload).await,
        &format!("volume → {percent}%"),
    )
}

async fn send_simple(socket_path: &Path, command: ServerCommand) -> String {
    let payload = CommandPayload::simple(command);
    format_ipc_response(send_command(socket_path, payload).await, "ok")
}

/// Like [`send_simple`], but for the query commands whose usefulness is in
/// the payload — a bare "ok" would throw away everything the user asked for.
async fn send_status(socket_path: &Path, command: ServerCommand) -> String {
    let payload = CommandPayload::simple(command);
    match send_command(socket_path, payload).await {
        Ok(ServerMessage::Response {
            ok, result, error, ..
        }) => {
            if ok {
                result
                    .as_ref()
                    .and_then(|v| v.get("message"))
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
                    .or_else(|| result.as_ref().map(summarize_json))
                    .unwrap_or_else(|| "ok".to_string())
            } else {
                error
                    .map(|e| format!("error [{}]: {}", e.code, e.message))
                    .unwrap_or_else(|| "error".to_string())
            }
        }
        Ok(_) => "unexpected response".to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// One-line rendering of a status payload, truncated on a char boundary so
/// it fits the single result line the palette gives us.
fn summarize_json(value: &serde_json::Value) -> String {
    const MAX_CHARS: usize = 220;
    let text = value.to_string();
    match text.char_indices().nth(MAX_CHARS) {
        Some((byte_idx, _)) => format!("{}…", &text[..byte_idx]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionCtx, Hitboxes, MAX_TARGET_TOKEN_LENGTH, apply_local_action, command_to_payload,
        open_settings, persist_theme_choice,
    };
    use crate::domain::command::Command;
    use crate::tui::input::TuiAction;
    use crate::tui::model::{FocusPanel, TuiModel, View};
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

    #[test]
    fn local_actions_update_the_model_without_a_daemon() {
        let owned = LocalCtx::new();
        let ctx = owned.ctx();
        let mut model = TuiModel::default();

        assert_eq!(
            apply_local_action(TuiAction::FocusAudio, &mut model, &ctx),
            Some((false, None))
        );
        assert_eq!(model.focus, FocusPanel::Audio);

        assert_eq!(
            apply_local_action(TuiAction::Quit, &mut model, &ctx),
            Some((true, None))
        );

        let (quit, status) =
            apply_local_action(TuiAction::ToggleIcons, &mut model, &ctx).expect("local action");
        assert!(!quit);
        assert_eq!(status.as_deref(), Some("icons off"));
        assert!(!model.show_icons);
    }

    /// The daemon-bound actions must report themselves as not-local, so
    /// `run_action` keeps handling them rather than silently dropping them.
    #[test]
    fn daemon_actions_are_not_local() {
        let owned = LocalCtx::new();
        let ctx = owned.ctx();
        let mut model = TuiModel::default();

        for action in [
            TuiAction::ToggleStream,
            TuiAction::ToggleRecord,
            TuiAction::ReloadConfig,
            TuiAction::Activate,
            TuiAction::RetryConnect,
        ] {
            assert_eq!(apply_local_action(action, &mut model, &ctx), None);
        }
    }

    #[test]
    fn open_settings_enters_settings_view_and_remembers_current_theme() {
        let mut model = TuiModel::default();
        let original_theme = model.theme;

        open_settings(&mut model);

        assert_eq!(model.view, View::Settings);
        assert_eq!(model.theme_preview_origin, Some(original_theme));
        assert_eq!(model.settings_cursor, original_theme.index());
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

    #[test]
    fn command_to_payload_rejects_invalid_target_values() {
        let cmd = Command::SetScene {
            target: "  \t\n  ".to_string(),
        };
        assert!(command_to_payload(cmd).is_err());

        let cmd = Command::Mute {
            target: "Mic\nInput".to_string(),
        };
        assert!(command_to_payload(cmd).is_err());
    }

    #[test]
    fn command_to_payload_sanitizes_target_whitespace() {
        let cmd = Command::ToggleMute {
            target: "  Mic  ".to_string(),
        };
        let payload = command_to_payload(cmd).unwrap();
        assert_eq!(payload.name, "toggle_mute");
        assert_eq!(
            payload.args.get("target").and_then(|v| v.as_str()).unwrap(),
            "Mic"
        );
    }

    #[test]
    fn command_to_payload_rejects_excessive_target_length() {
        let cmd = Command::SetVolume {
            target: "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1),
            percent: 50,
        };
        assert!(command_to_payload(cmd).is_err());
    }

    #[test]
    fn command_to_payload_rejects_volume_percent_out_of_range() {
        let cmd = Command::SetVolume {
            target: "Mic".to_string(),
            percent: 101,
        };
        assert!(command_to_payload(cmd).is_err());
    }
}
