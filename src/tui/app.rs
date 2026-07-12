use std::{io::stdout, path::Path, time::Duration};

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
    domain::{command::Command, parser, result::Result},
    ipc::protocol::{CommandPayload, LogLevel, ServerMessage},
    support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len},
    tui::{
        completion,
        event_applier::apply_server_message,
        input::{TuiAction, handle_key},
        layout,
        model::{FocusPanel, TuiLogEntry, TuiModel},
        session::{TuiEventSession, send_command},
        theme::Theme,
        widgets,
    },
};

pub async fn run(socket_path: &Path, refresh_ms: u64, theme_id: &str) -> Result<i32> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, socket_path, refresh_ms, theme_id).await;

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    socket_path: &Path,
    refresh_ms: u64,
    theme_id: &str,
) -> Result<i32> {
    let mut model = TuiModel::with_theme(Theme::by_id(theme_id));
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
            model.last_result = Some(msg.clone());
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
                terminal.draw(|f| render(f, &model))?;
            }

            maybe_ipc = ipc_rx.recv() => {
                let needs_redraw = match maybe_ipc {
                    Some(Ok(msg)) => apply_server_message(&mut model, msg),
                    Some(Err(e)) => {
                        model.connected_to_daemon = false;
                        model.last_result = Some(format!("Daemon disconnected: {e}"));
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
                            let (should_quit, result) =
                                handle_action(action, &mut model, socket_path, &ipc_tx).await;
                            if should_quit {
                                return Ok(0);
                            }
                            if let Some(r) = result {
                                model.last_result = Some(r);
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
    }
}

fn render(f: &mut ratatui::Frame, model: &TuiModel) {
    let areas = layout::compute(f);

    if !model.connected_to_daemon {
        widgets::connection::render_unavailable(f, f.area(), model);
        return;
    }

    widgets::header::render(f, areas.header, model);
    widgets::scenes::render(f, areas.left_top, model);
    widgets::audio::render(f, areas.left_bottom, model);
    widgets::logs::render(f, areas.right, model);
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
        Ok(Command::Help) => {
            "Commands: /scene /mute /unmute /toggle-mute /vol /stream /rec /status \
             /obs-status /server-status /reload-config /dump-config /validate-config \
             /reconnect /quit"
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
        Command::Help | Command::Quit => unreachable!("handled before"),
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
    use super::{MAX_TARGET_TOKEN_LENGTH, command_to_payload};
    use crate::domain::command::Command;

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
