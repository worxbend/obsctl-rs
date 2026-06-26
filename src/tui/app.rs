use std::{io::stdout, path::Path, time::Duration};

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
    ipc::protocol::{CommandPayload, ServerMessage},
    tui::{
        event_applier::apply_server_message,
        input::{TuiAction, handle_key},
        layout,
        model::{FocusPanel, TuiModel},
        session::{TuiEventSession, send_command},
        widgets,
    },
};

pub async fn run(socket_path: &Path, refresh_ms: u64) -> Result<i32> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, socket_path, refresh_ms).await;

    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;

    result
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    socket_path: &Path,
    refresh_ms: u64,
) -> Result<i32> {
    let mut model = TuiModel::default();
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
            model.last_result = Some(format!("Cannot connect to daemon: {e}"));
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
            (false, None)
        }
        TuiAction::ClosePalette => {
            model.command_palette.active = false;
            model.command_palette.input.clear();
            (false, None)
        }
        TuiAction::PaletteChar(c) => {
            model.command_palette.input.push(c);
            (false, None)
        }
        TuiAction::PaletteBackspace => {
            model.command_palette.input.pop();
            (false, None)
        }
        TuiAction::PaletteSubmit => {
            let input = model.command_palette.input.clone();
            model.command_palette.active = false;
            model.command_palette.input.clear();
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
        TuiAction::RetryConnect => {
            match TuiEventSession::connect(socket_path).await {
                Ok(session) => {
                    model.connected_to_daemon = true;
                    spawn_session_forwarder(session, ipc_tx.clone());
                    (false, Some("Reconnected to daemon.".to_string()))
                }
                Err(e) => (false, Some(format!("Retry failed: {e}"))),
            }
        }
    }
}

fn render(f: &mut ratatui::Frame, model: &TuiModel) {
    let areas = layout::compute(f);

    if !model.connected_to_daemon {
        render_unavailable(f, f.area(), model);
        return;
    }

    widgets::header::render(f, areas.header, model);
    widgets::scenes::render(f, areas.left_top, model);
    widgets::audio::render(f, areas.left_bottom, model);
    widgets::logs::render(f, areas.right, model);
    widgets::command_palette::render(f, areas.palette, model);
}

fn render_unavailable(f: &mut ratatui::Frame, area: ratatui::layout::Rect, model: &TuiModel) {
    use ratatui::{
        style::{Color, Style},
        text::Line,
        widgets::{Block, Borders, Paragraph},
    };

    let err = model
        .last_result
        .as_deref()
        .unwrap_or("Could not connect to obsctl daemon.");

    let lines = vec![
        Line::styled(
            "obsctl server is not running",
            Style::default().fg(Color::Red),
        ),
        Line::raw(""),
        Line::styled(err, Style::default().fg(Color::Yellow)),
        Line::raw(""),
        Line::raw("Start the daemon with:"),
        Line::styled(
            "  obsctl server --headless",
            Style::default().fg(Color::Cyan),
        ),
        Line::raw("Or install the service:"),
        Line::styled("  obsctl service install", Style::default().fg(Color::Cyan)),
        Line::styled(
            "  systemctl --user enable --now obsctl.service",
            Style::default().fg(Color::Cyan),
        ),
        Line::raw(""),
        Line::styled(
            "Press R to retry, q to quit.",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" obsctl — daemon unavailable ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
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
            let payload = command_to_payload(cmd);
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

fn command_to_payload(cmd: Command) -> CommandPayload {
    let (name, args) = match cmd {
        Command::Status => ("get_snapshot", serde_json::Value::Null),
        Command::ServerStatus => ("get_server_status", serde_json::Value::Null),
        Command::ObsStatus => ("get_obs_status", serde_json::Value::Null),
        Command::ValidateConfig => ("validate_config", serde_json::Value::Null),
        Command::Reconnect | Command::Connect => ("reconnect_obs", serde_json::Value::Null),
        Command::ShutdownServer => ("shutdown_server", serde_json::Value::Null),
        Command::DumpConfig => ("dump_config", serde_json::Value::Null),
        Command::ReloadConfig => ("reload_config", serde_json::Value::Null),
        Command::SetScene { target } => ("set_scene", serde_json::json!({ "target": target })),
        Command::Mute { target } => ("mute", serde_json::json!({ "target": target })),
        Command::Unmute { target } => ("unmute", serde_json::json!({ "target": target })),
        Command::ToggleMute { target } => ("toggle_mute", serde_json::json!({ "target": target })),
        Command::SetVolume { target, percent } => (
            "set_volume",
            serde_json::json!({ "target": target, "percent": percent }),
        ),
        Command::ToggleStream => ("toggle_stream", serde_json::Value::Null),
        Command::ToggleRecord => ("toggle_record", serde_json::Value::Null),
        Command::Help | Command::Quit => unreachable!("handled before"),
    };
    CommandPayload {
        name: name.to_string(),
        args,
    }
}

fn format_ipc_response(res: Result<ServerMessage>, ok_fallback: &str) -> String {
    match res {
        Ok(ServerMessage::Response { ok, result, error, .. }) => {
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
    let payload = CommandPayload {
        name: name.to_string(),
        args: serde_json::json!({ "target": target }),
    };
    format_ipc_response(send_command(socket_path, payload).await, "ok")
}

async fn send_set_volume(socket_path: &Path, target: &str, percent: u8) -> String {
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
