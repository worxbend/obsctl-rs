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
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

use crate::{
    domain::result::Result,
    ipc::protocol::{LogLevel, ServerMessage},
    tui::{
        actions::{ActionCtx, ActionOutcome, handle_action},
        daemon::send_set_volume,
        event_applier::apply_server_message,
        input::handle_key,
        model::{DEFAULT_PALETTE_PREFIX, TuiLogEntry, TuiModel},
        mouse::{self, Hitboxes},
        render::render,
        session::TuiEventSession,
        theme::Theme,
        widgets,
    },
};

/// Total time the startup splash is shown, unless skipped by a keypress.
const SPLASH_DURATION: Duration = Duration::from_millis(2000);
const SPLASH_FRAME_INTERVAL: Duration = Duration::from_millis(50);

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

/// Owns the terminal's alternate screen, raw mode, and mouse reporting for as
/// long as the TUI is running, and gives them back when it is dropped.
///
/// The teardown used to be three statements after the event loop. Anything
/// that left `run` earlier — the `?` on building the `Terminal`, the `?` on
/// the splash, or a panic anywhere in the loop — skipped all three and handed
/// the user back a terminal still in raw mode on the alternate screen, with
/// mouse reporting on: no echo, no working Ctrl-C, and their shell prompt
/// hidden. Recovering meant knowing to type `reset` blind. Unwinding runs a
/// `Drop`, so putting the teardown here covers every exit.
struct TerminalGuard {
    mouse: bool,
}

impl TerminalGuard {
    /// Put the terminal into the mode the TUI needs.
    ///
    /// The guard is created first and each step applied after, so a failure
    /// part-way through is still undone by the drop.
    fn acquire(mouse: bool) -> Result<Self> {
        let guard = Self { mouse };
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        if mouse {
            execute!(stdout(), EnableMouseCapture)?;
        }
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best effort throughout: this runs during unwinding as well as on the
        // normal path, and there is nowhere to report a failure to. Leaving a
        // later step undone because an earlier one failed would be worse than
        // trying them all.
        if self.mouse {
            let _ = execute!(stdout(), DisableMouseCapture);
        }
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

pub async fn run(socket_path: &Path, options: TuiOptions) -> Result<i32> {
    let _terminal_guard = TerminalGuard::acquire(options.mouse)?;

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

    run_loop(&mut terminal, socket_path, &options).await
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

/// The channels the event loop selects over, opened together by
/// [`start_session`] because they are only meaningful as a set.
struct LoopChannels {
    /// Kept alongside the receiver so a reconnect can hand a fresh forwarder
    /// task somewhere to send.
    ipc_tx: mpsc::Sender<std::result::Result<ServerMessage, String>>,
    ipc_rx: mpsc::Receiver<std::result::Result<ServerMessage, String>>,
    /// Desired volume per input, drained by the debouncer task.
    vol_tx: mpsc::UnboundedSender<(String, u8)>,
    /// What the debouncer made of each `set_volume` it ended up sending.
    status_rx: mpsc::Receiver<String>,
}

/// Build the model, open the channels, start the background tasks, and make
/// the first attempt at reaching the daemon.
///
/// Split out so that [`run_loop`]'s body is the loop and nothing else — the
/// setup shares nothing with the per-event policy below it beyond what it
/// returns here.
async fn start_session(options: &TuiOptions, socket_path: &Path) -> (TuiModel, LoopChannels) {
    let mut model =
        TuiModel::with_appearance(options.theme, options.show_icons, options.advanced_ui);
    model.palette_prefix = options.palette_prefix;

    let (ipc_tx, ipc_rx) = mpsc::channel::<std::result::Result<ServerMessage, String>>(64);

    // Volume changes are applied to the model optimistically on every keypress
    // and the latest target per input is forwarded to a background debouncer,
    // so the event loop never blocks on the IPC/OBS round-trip and OBS isn't
    // flooded with intermediate steps during a rapid up/down burst. The
    // debouncer reports each command's outcome back through `status_rx`.
    let (vol_tx, vol_rx) = mpsc::unbounded_channel::<(String, u8)>();
    let (status_tx, status_rx) = mpsc::channel::<String>(16);
    spawn_volume_debouncer(socket_path.to_path_buf(), vol_rx, status_tx);

    match TuiEventSession::connect(socket_path).await {
        Ok(session) => {
            model.connected_to_daemon = true;
            spawn_session_forwarder(session, ipc_tx.clone());
        }
        Err(e) => report_connect_failure(&mut model, e),
    }

    (
        model,
        LoopChannels {
            ipc_tx,
            ipc_rx,
            vol_tx,
            status_rx,
        },
    )
}

/// Starting with no daemon is normal — the TUI shows a "daemon unavailable"
/// screen and a click retries — so the failure is reported in both places the
/// user looks (the status line and the log pane) rather than aborting startup.
fn report_connect_failure(model: &mut TuiModel, error: impl std::fmt::Display) {
    model.connected_to_daemon = false;
    let msg = format!("Cannot connect to daemon: {error}");
    model.set_last_result(msg.clone());
    model.push_log(TuiLogEntry {
        level: LogLevel::Error,
        message: msg,
        target: None,
        timestamp: OffsetDateTime::now_utc(),
    });
}

/// What one pass through the event loop decided.
///
/// The two exits used to be `return`s from inside `tokio::select!` arms, two
/// levels down inside a macro, which is an easy pair to miss when reading for
/// "how does this loop end?". Each arm now answers with a value and the loop
/// itself has one exit.
enum Flow {
    /// Keep going. `redraw` is this arm's answer to the loop's one redraw
    /// question, asked and acted on in a single place at the bottom.
    Continue { redraw: bool },
    /// Leave the TUI with this process exit code.
    Quit(i32),
}

impl Flow {
    fn redraw() -> Self {
        Self::Continue { redraw: true }
    }

    fn idle() -> Self {
        Self::Continue { redraw: false }
    }

    fn redraw_if(redraw: bool) -> Self {
        Self::Continue { redraw }
    }
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    socket_path: &Path,
    options: &TuiOptions,
) -> Result<i32> {
    let config_path = options.config_path.clone();
    let refresh = Duration::from_millis(options.refresh_ms.max(50));
    // Where the last frame put each panel; mouse events resolve against it.
    let mut hits = Hitboxes::default();

    let (mut model, mut channels) = start_session(options, socket_path).await;

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(refresh);

    loop {
        let flow = tokio::select! {
            _ = ticker.tick() => {
                model.anim.tick();
                Flow::redraw()
            }

            maybe_ipc = channels.ipc_rx.recv() => {
                match maybe_ipc {
                    Some(Ok(msg)) => Flow::redraw_if(apply_server_message(&mut model, msg)),
                    Some(Err(e)) => {
                        model.connected_to_daemon = false;
                        model.set_last_result(format!("Daemon disconnected: {e}"));
                        Flow::redraw()
                    }
                    None => {
                        model.connected_to_daemon = false;
                        Flow::redraw()
                    }
                }
            }

            maybe_status = channels.status_rx.recv() => {
                match maybe_status {
                    Some(status) => {
                        model.set_last_result(status);
                        Flow::redraw()
                    }
                    None => Flow::idle(),
                }
            }

            maybe_event = events.next() => {
                let ctx = ActionCtx {
                    socket_path,
                    config_path: config_path.as_deref(),
                    ipc_tx: &channels.ipc_tx,
                    vol_tx: &channels.vol_tx,
                    hits: &hits,
                };
                handle_terminal_event(maybe_event, &mut model, &ctx).await
            }
        };

        // One redraw rule, in one place: the arms decide *whether* the screen
        // changed, and only this line puts a frame on it.
        match flow {
            Flow::Quit(code) => return Ok(code),
            Flow::Continue { redraw } => {
                if redraw {
                    draw(terminal, &model, &mut hits)?;
                }
            }
        }
    }
}

/// Turn one event from the terminal into the loop's next step.
///
/// Keeps the loop body flat: the nesting of "which event, then which action,
/// then what did the action say" lives here rather than three levels deep
/// inside the `select!` macro.
async fn handle_terminal_event(
    event: Option<std::io::Result<Event>>,
    model: &mut TuiModel,
    ctx: &ActionCtx<'_>,
) -> Flow {
    let event = match event {
        // A resize changes nothing in the model, but the frame that was drawn
        // for the old size no longer fits it.
        Some(Ok(Event::Resize(_, _))) => return Flow::redraw(),
        // The terminal's event stream ended or broke: nobody is left to drive
        // the TUI, so stop with a failure code.
        None | Some(Err(_)) => return Flow::Quit(1),
        Some(Ok(event)) => event,
    };

    let action = match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_key(model, key),
        Event::Mouse(m) => mouse::handle_mouse(model, ctx.hits, m),
        _ => None,
    };

    // A key that is bound to nothing leaves the screen exactly as it was.
    let Some(action) = action else {
        return Flow::idle();
    };

    match handle_action(action, model, ctx).await {
        ActionOutcome::Quit => Flow::Quit(0),
        ActionOutcome::Status(status) => {
            model.set_last_result(status);
            Flow::redraw()
        }
        ActionOutcome::Continue => Flow::redraw(),
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

pub(super) fn spawn_session_forwarder(
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
