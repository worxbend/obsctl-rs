use std::{
    io::stdout,
    path::{Path, PathBuf},
    time::Duration,
};

use time::OffsetDateTime;

use crossterm::{
    event::{Event, EventStream, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::{
    config::{loader, writer},
    domain::{command::Command, parser, result::Result},
    ipc::protocol::{CommandPayload, LogLevel, ServerMessage},
    support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len},
    tui::{
        completion,
        event_applier::apply_server_message,
        input::{TuiAction, handle_key},
        layout,
        model::{FocusPanel, TuiLogEntry, TuiModel, View},
        session::{TuiEventSession, send_command},
        theme::{self, Theme},
        widgets,
    },
};

/// Total time the startup splash is shown, unless skipped by a keypress.
const SPLASH_DURATION: Duration = Duration::from_millis(2000);
const SPLASH_FRAME_INTERVAL: Duration = Duration::from_millis(50);

pub async fn run(
    socket_path: &Path,
    refresh_ms: u64,
    theme: Theme,
    config_path: Option<PathBuf>,
) -> Result<i32> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    {
        let mut splash_events = EventStream::new();
        run_splash(&mut terminal, &mut splash_events, theme).await?;
    }

    let result = run_loop(&mut terminal, socket_path, refresh_ms, theme, config_path).await;

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
) -> Result<()> {
    let total_frames =
        (SPLASH_DURATION.as_millis() / SPLASH_FRAME_INTERVAL.as_millis().max(1)) as u64;
    let mut ticker = tokio::time::interval(SPLASH_FRAME_INTERVAL);
    let mut frame = 0u64;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                terminal.draw(|f| widgets::splash::render(f, theme, frame, total_frames))?;
                if frame >= total_frames {
                    return Ok(());
                }
                frame += 1;
            }
            maybe_event = events.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event
                    && key.kind == KeyEventKind::Press
                {
                    return Ok(());
                }
            }
        }
    }
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    socket_path: &Path,
    refresh_ms: u64,
    theme: Theme,
    config_path: Option<PathBuf>,
) -> Result<i32> {
    let mut model = TuiModel::with_theme(theme);
    let refresh = Duration::from_millis(refresh_ms.max(50));

    // Try to connect to daemon
    let (ipc_tx, mut ipc_rx) = mpsc::channel::<std::result::Result<ServerMessage, String>>(64);

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
                terminal.draw(|f| render(f, &model))?;
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
                    terminal.draw(|f| render(f, &model))?;
                }
            }

            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        if let Some(action) = handle_key(&model, key) {
                            let (should_quit, result) = handle_action(
                                action,
                                &mut model,
                                socket_path,
                                config_path.as_deref(),
                                &ipc_tx,
                            )
                            .await;
                            if should_quit {
                                return Ok(0);
                            }
                            if let Some(r) = result {
                                model.set_last_result(r);
                            }
                            terminal.draw(|f| render(f, &model))?;
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        terminal.draw(|f| render(f, &model))?;
                    }
                    None | Some(Err(_)) => return Ok(1),
                    _ => {}
                }
            }
        }
    }
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

async fn handle_action(
    action: TuiAction,
    model: &mut TuiModel,
    socket_path: &Path,
    config_path: Option<&Path>,
    ipc_tx: &mpsc::Sender<std::result::Result<ServerMessage, String>>,
) -> (bool, Option<String>) {
    match action {
        TuiAction::Quit => (true, None),
        TuiAction::OpenPalette => {
            model.command_palette.active = true;
            model.command_palette.input.clear();
            model.command_palette.input.push('/');
            refresh_completions(model);
            (false, None)
        }
        TuiAction::ClosePalette => {
            model.command_palette.active = false;
            model.command_palette.input.clear();
            model.command_palette.completions.clear();
            model.command_palette.completion_idx = None;
            (false, None)
        }
        TuiAction::PaletteChar(c) => {
            model.command_palette.input.push(c);
            refresh_completions(model);
            (false, None)
        }
        TuiAction::PaletteBackspace => {
            model.command_palette.input.pop();
            refresh_completions(model);
            (false, None)
        }
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
        TuiAction::ReloadConfig => {
            let result = send_simple(socket_path, "reload_config").await;
            (false, Some(result))
        }
        TuiAction::DumpConfig => {
            let result = send_simple(socket_path, "dump_config").await;
            (false, Some(result))
        }
        TuiAction::FocusScenes => {
            model.focus = FocusPanel::Scenes;
            (false, None)
        }
        TuiAction::FocusAudio => {
            model.focus = FocusPanel::Audio;
            (false, None)
        }
        TuiAction::FocusProfiles => {
            model.focus = FocusPanel::Profiles;
            (false, None)
        }
        TuiAction::NavUp => {
            model.move_up();
            (false, None)
        }
        TuiAction::NavDown => {
            model.move_down();
            (false, None)
        }
        TuiAction::ActivateScene => {
            if let Some(name) = model.focused_scene().map(|s| s.name.clone()) {
                let result = send_simple_with_target(socket_path, "set_scene", &name).await;
                (false, Some(result))
            } else {
                (false, None)
            }
        }
        TuiAction::ActivateProfile => {
            if let Some(name) = model.focused_profile().map(str::to_string) {
                let result = send_simple_with_target(socket_path, "set_profile", &name).await;
                (false, Some(result))
            } else {
                (false, None)
            }
        }
        TuiAction::ToggleMute => {
            if let Some(name) = model.focused_audio().map(|a| a.name.clone()) {
                let result = send_simple_with_target(socket_path, "toggle_mute", &name).await;
                (false, Some(result))
            } else {
                (false, None)
            }
        }
        TuiAction::VolumeDown => {
            if let Some(a) = model.focused_audio() {
                let name = a.name.clone();
                let current = a.volume_percent.unwrap_or(50);
                let new_vol = current.saturating_sub(5);
                let result = send_set_volume(socket_path, &name, new_vol).await;
                (false, Some(result))
            } else {
                (false, None)
            }
        }
        TuiAction::VolumeUp => {
            if let Some(a) = model.focused_audio() {
                let name = a.name.clone();
                let current = a.volume_percent.unwrap_or(50);
                let new_vol = (current + 5).min(100);
                let result = send_set_volume(socket_path, &name, new_vol).await;
                (false, Some(result))
            } else {
                (false, None)
            }
        }
        TuiAction::RetryConnect => match TuiEventSession::connect(socket_path).await {
            Ok(session) => {
                model.connected_to_daemon = true;
                spawn_session_forwarder(session, ipc_tx.clone());
                (false, Some("Reconnected to daemon.".to_string()))
            }
            Err(e) => (false, Some(format!("Retry failed: {e}"))),
        },
        TuiAction::CompleteNext => {
            model.command_palette.cycle_next();
            (false, None)
        }
        TuiAction::CompletePrev => {
            model.command_palette.cycle_prev();
            (false, None)
        }
        TuiAction::OpenSettings => {
            open_settings(model);
            (false, None)
        }
        TuiAction::CloseSettings => {
            if let Some(original) = model.theme_preview_origin.take() {
                model.theme = original;
            }
            model.view = View::Main;
            (false, None)
        }
        TuiAction::SettingsNavUp => {
            model.settings_cursor = model.settings_cursor.saturating_sub(1);
            model.theme = theme::ALL[model.settings_cursor];
            (false, None)
        }
        TuiAction::SettingsNavDown => {
            let max = theme::ALL.len().saturating_sub(1);
            model.settings_cursor = (model.settings_cursor + 1).min(max);
            model.theme = theme::ALL[model.settings_cursor];
            (false, None)
        }
        TuiAction::ApplySettingsTheme => {
            let chosen = theme::ALL[model.settings_cursor];
            model.theme = chosen;
            model.theme_preview_origin = None;
            model.view = View::Main;
            let result = persist_theme_choice(config_path, chosen.id).await;
            (false, Some(result))
        }
    }
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

fn render(f: &mut ratatui::Frame, model: &TuiModel) {
    if model.view == View::Settings {
        widgets::settings::render(f, f.area(), model);
        return;
    }

    let areas = layout::compute(f);

    if !model.connected_to_daemon {
        widgets::connection::render_unavailable(f, f.area(), model);
        return;
    }

    widgets::header::render(f, areas.header, model);
    widgets::live_bar::render(f, areas.live_bar, model);
    widgets::scenes::render(f, areas.scenes, model);
    widgets::audio::render(f, areas.audio, model);
    widgets::profiles::render(f, areas.profiles, model);
    widgets::logs::render(f, areas.logs, model);
    widgets::command_palette::render(f, areas.palette, model);
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
            "Commands: /scene /profile /mute /unmute /toggle-mute /vol /stream /rec /status \
             /obs-status /server-status /reload-config /dump-config /validate-config \
             /themes /reconnect /quit"
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
    let (name, args) = match cmd {
        Command::Status => ("get_snapshot", serde_json::Value::Null),
        Command::ServerStatus => ("get_server_status", serde_json::Value::Null),
        Command::ObsStatus => ("get_obs_status", serde_json::Value::Null),
        Command::ValidateConfig => ("validate_config", serde_json::Value::Null),
        Command::Reconnect | Command::Connect => ("reconnect_obs", serde_json::Value::Null),
        Command::ShutdownServer => ("shutdown_server", serde_json::Value::Null),
        Command::DumpConfig => ("dump_config", serde_json::Value::Null),
        Command::ReloadConfig => ("reload_config", serde_json::Value::Null),
        Command::SetScene { target } => {
            let target = sanitize_target_arg(&target)?;
            ("set_scene", serde_json::json!({ "target": target }))
        }
        Command::SetProfile { target } => {
            let target = sanitize_target_arg(&target)?;
            ("set_profile", serde_json::json!({ "target": target }))
        }
        Command::Mute { target } => {
            let target = sanitize_target_arg(&target)?;
            ("mute", serde_json::json!({ "target": target }))
        }
        Command::Unmute { target } => {
            let target = sanitize_target_arg(&target)?;
            ("unmute", serde_json::json!({ "target": target }))
        }
        Command::ToggleMute { target } => {
            let target = sanitize_target_arg(&target)?;
            ("toggle_mute", serde_json::json!({ "target": target }))
        }
        Command::SetVolume { target, percent } => {
            if percent > 100 {
                return Err("volume percent must be 0-100".to_string());
            }
            (
                "set_volume",
                serde_json::json!({
                    "target": sanitize_target_arg(&target)?,
                    "percent": percent
                }),
            )
        }
        Command::ToggleStream => ("toggle_stream", serde_json::Value::Null),
        Command::ToggleRecord => ("toggle_record", serde_json::Value::Null),
        Command::Help | Command::Quit | Command::Themes => unreachable!("handled before"),
    };
    Ok(CommandPayload {
        name: name.to_string(),
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

async fn send_simple_with_target(socket_path: &Path, name: &str, target: &str) -> String {
    let target = match sanitize_target_arg(target) {
        Ok(target) => target,
        Err(error) => return format!("error: invalid target: {error}"),
    };

    let payload = CommandPayload {
        name: name.to_string(),
        args: serde_json::json!({ "target": target }),
    };
    format_ipc_response(send_command(socket_path, payload).await, "ok")
}

async fn send_set_volume(socket_path: &Path, target: &str, percent: u8) -> String {
    let target = match sanitize_target_arg(target) {
        Ok(target) => target,
        Err(error) => return format!("error: invalid target: {error}"),
    };
    if percent > 100 {
        return "error: volume percent must be 0-100".to_string();
    }

    let payload = CommandPayload {
        name: "set_volume".to_string(),
        args: serde_json::json!({ "target": target, "percent": percent }),
    };
    format_ipc_response(
        send_command(socket_path, payload).await,
        &format!("volume → {percent}%"),
    )
}

async fn send_simple(socket_path: &Path, name: &str) -> String {
    let payload = CommandPayload {
        name: name.to_string(),
        args: serde_json::Value::Null,
    };
    format_ipc_response(send_command(socket_path, payload).await, "ok")
}

#[cfg(test)]
mod tests {
    use super::{MAX_TARGET_TOKEN_LENGTH, command_to_payload, open_settings, persist_theme_choice};
    use crate::domain::command::Command;
    use crate::tui::model::{TuiModel, View};

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
