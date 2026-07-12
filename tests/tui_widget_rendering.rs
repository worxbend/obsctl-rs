// TUI widget rendering tests using Ratatui's TestBackend.
// These tests verify that widgets render expected text without panicking
// across connected, disconnected, empty, and error states.

use obsctl_rs::{
    ipc::protocol::LogLevel,
    obs::state::{AudioState, ObsSnapshot, SceneState},
    tui::{
        model::{TuiLogEntry, TuiModel},
        widgets,
    },
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use time::OffsetDateTime;

fn term(width: u16, height: u16) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(width, height)).expect("terminal")
}

fn snap_connected() -> ObsSnapshot {
    ObsSnapshot {
        connected: true,
        obs_studio_version: Some("30.1.0".into()),
        obs_websocket_version: Some("5.0.0".into()),
        current_scene: Some("Main".into()),
        scenes: vec![
            SceneState {
                name: "Main".into(),
                alias: Some("m".into()),
                shortcut: Some("1".into()),
                group: None,
                active: true,
                hidden: false,
            },
            SceneState {
                name: "Cam".into(),
                alias: None,
                shortcut: None,
                group: None,
                active: false,
                hidden: false,
            },
        ],
        audio_inputs: vec![
            AudioState {
                name: "Mic".into(),
                alias: Some("mic".into()),
                shortcut: None,
                kind: Some("wasapi_input_capture".into()),
                muted: Some(false),
                volume_percent: Some(80),
                volume_mul: None,
                volume_db: None,
            },
            AudioState {
                name: "Desktop".into(),
                alias: None,
                shortcut: None,
                kind: None,
                muted: Some(true),
                volume_percent: Some(0),
                volume_mul: None,
                volume_db: None,
            },
        ],
        streaming: false,
        recording: false,
        profiles: vec!["Default".into(), "Streaming".into()],
        current_profile: Some("Default".into()),
        updated_at: OffsetDateTime::now_utc(),
        ..ObsSnapshot::default()
    }
}

fn snap_disconnected_with_error() -> ObsSnapshot {
    ObsSnapshot {
        last_error: Some("connection refused".into()),
        updated_at: OffsetDateTime::now_utc(),
        ..ObsSnapshot::default()
    }
}

fn log_entry(level: LogLevel, message: &str) -> TuiLogEntry {
    TuiLogEntry {
        level,
        message: message.into(),
        target: Some("obsctl_rs::server".into()),
        timestamp: OffsetDateTime::UNIX_EPOCH,
    }
}

fn model_connected() -> TuiModel {
    let mut model = TuiModel::default();
    model.snapshot = Some(snap_connected());
    model.logs = vec![
        log_entry(LogLevel::Info, "server started"),
        log_entry(LogLevel::Warn, "reconnect attempt"),
    ];
    model.connected_to_daemon = true;
    model.clamp_cursors();
    model
}

fn model_daemon_disconnected() -> TuiModel {
    let mut model = TuiModel::default();
    model.connected_to_daemon = false;
    model
}

fn buf_string(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer().clone();
    let width = buf.area().width as usize;
    let height = buf.area().height as usize;
    let mut out = String::new();
    for y in 0..height {
        for x in 0..width {
            let cell = buf.cell((x as u16, y as u16)).unwrap();
            out.push_str(cell.symbol());
        }
        out.push('\n');
    }
    out
}

// ── background fill ─────────────────────────────────────────────────────────

#[test]
fn fill_background_paints_every_cell_with_theme_background() {
    use obsctl_rs::tui::theme::Theme;

    let theme = Theme::by_id("nord");
    let mut t = term(20, 6);
    t.draw(|f| {
        widgets::fill_background(f, theme);
    })
    .unwrap();

    let buf = t.backend().buffer().clone();
    for y in 0..buf.area().height {
        for x in 0..buf.area().width {
            let cell = buf.cell((x, y)).unwrap();
            assert_eq!(cell.style().bg, Some(theme.bg));
        }
    }
}

// ── header widget ──────────────────────────────────────────────────────────

#[test]
fn header_renders_connected_state() {
    let model = model_connected();
    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::header::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("obsctl"), "should contain app name");
    assert!(out.contains("connected"), "should show connected state");
    assert!(out.contains("OBS:"), "should show OBS status");
}

#[test]
fn header_renders_current_profile() {
    let model = model_connected();
    let mut t = term(110, 5);
    t.draw(|f| {
        widgets::header::render(f, Rect::new(0, 0, 110, 5), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("profile: Default"), "should show profile name");
}

#[test]
fn header_renders_daemon_disconnected() {
    let model = model_daemon_disconnected();
    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::header::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("disconnected"), "should show disconnected");
}

// ── live bar widget ─────────────────────────────────────────────────────────

#[test]
fn live_bar_renders_off_state_and_stats_placeholder() {
    let model = model_connected();
    let mut t = term(100, 3);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 3), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("LIVE off"), "should show LIVE off badge");
    assert!(out.contains("REC off"), "should show REC off badge");
    assert!(
        out.contains("waiting"),
        "should show stats placeholder when no stats yet; got: {out}"
    );
}

#[test]
fn live_bar_renders_active_stream_and_record_with_stats() {
    let mut model = model_connected();
    if let Some(snap) = model.snapshot.as_mut() {
        snap.streaming = true;
        snap.recording = true;
        snap.stream_duration_ms = Some(65_000);
        snap.record_duration_ms = Some(5_000);
        snap.stream_bitrate_kbps = Some(4500.0);
        snap.stats = Some(obsctl_rs::obs::state::ObsStats {
            cpu_usage_percent: 12.5,
            active_fps: 60.0,
            memory_usage_mb: 512.0,
            ..Default::default()
        });
    }
    let mut t = term(100, 3);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 3), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("LIVE"), "should show LIVE badge");
    assert!(out.contains("01:05"), "should show stream duration");
    assert!(out.contains("00:05"), "should show record duration");
    assert!(out.contains("4500"), "should show bitrate");
    assert!(out.contains("60.0"), "should show fps");
}

// ── connection widget ───────────────────────────────────────────────────────

#[test]
fn connection_renders_not_connected_to_daemon() {
    let model = model_daemon_disconnected();
    let mut t = term(60, 6);
    t.draw(|f| {
        widgets::connection::render(f, Rect::new(0, 0, 60, 6), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("Not connected"),
        "should show not connected message; got: {out}"
    );
}

#[test]
fn connection_renders_obs_disconnected_with_error() {
    let mut model = model_connected();
    model.snapshot = Some(snap_disconnected_with_error());
    let mut t = term(60, 6);
    t.draw(|f| {
        widgets::connection::render(f, Rect::new(0, 0, 60, 6), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("connection refused") || out.contains("last error"),
        "should show last error; got: {out}"
    );
}

#[test]
fn connection_renders_waiting_when_no_snapshot() {
    let mut model = model_connected();
    model.snapshot = None;
    let mut t = term(60, 6);
    t.draw(|f| {
        widgets::connection::render(f, Rect::new(0, 0, 60, 6), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("Waiting"),
        "should show waiting message; got: {out}"
    );
}

// ── scenes widget ───────────────────────────────────────────────────────────

#[test]
fn scenes_renders_active_scene_marker() {
    let model = model_connected();
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::scenes::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Scenes"), "should show Scenes title");
    assert!(out.contains("Main"), "should show scene name");
    assert!(out.contains("▶"), "should show active marker");
}

#[test]
fn scenes_renders_empty_list_without_panic() {
    let mut model = model_connected();
    if let Some(ref mut snap) = model.snapshot {
        snap.scenes.clear();
    }
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::scenes::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Scenes"), "should still render block title");
}

#[test]
fn scenes_renders_long_name_without_panic() {
    let mut model = model_connected();
    if let Some(ref mut snap) = model.snapshot {
        snap.scenes = vec![SceneState {
            name: "A".repeat(200),
            active: true,
            ..Default::default()
        }];
    }
    let mut t = term(40, 8);
    // This must not panic on narrow terminals.
    t.draw(|f| {
        widgets::scenes::render(f, Rect::new(0, 0, 40, 8), &model);
    })
    .unwrap();
}

#[test]
fn scenes_renders_alias_and_shortcut() {
    let model = model_connected();
    let mut t = term(80, 10);
    t.draw(|f| {
        widgets::scenes::render(f, Rect::new(0, 0, 80, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("(m)") || out.contains("m"),
        "should render alias"
    );
    assert!(
        out.contains("[1]") || out.contains("1"),
        "should render shortcut"
    );
}

// ── audio widget ────────────────────────────────────────────────────────────

#[test]
fn audio_renders_mute_and_volume() {
    let model = model_connected();
    let mut t = term(80, 10);
    t.draw(|f| {
        widgets::audio::render(f, Rect::new(0, 0, 80, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Audio"), "should show Audio title");
    assert!(out.contains("Mic"), "should show audio input name");
    assert!(
        out.contains("80%") || out.contains("80"),
        "should show volume"
    );
}

#[test]
fn audio_renders_empty_list_without_panic() {
    let mut model = model_connected();
    if let Some(ref mut snap) = model.snapshot {
        snap.audio_inputs.clear();
    }
    let mut t = term(60, 8);
    t.draw(|f| {
        widgets::audio::render(f, Rect::new(0, 0, 60, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Audio"), "should still render block title");
}

#[test]
fn audio_renders_long_name_without_panic() {
    let mut model = model_connected();
    if let Some(ref mut snap) = model.snapshot {
        snap.audio_inputs = vec![AudioState {
            name: "Z".repeat(200),
            muted: Some(false),
            volume_percent: Some(100),
            ..Default::default()
        }];
    }
    let mut t = term(30, 6);
    t.draw(|f| {
        widgets::audio::render(f, Rect::new(0, 0, 30, 6), &model);
    })
    .unwrap();
}

// ── profiles widget ─────────────────────────────────────────────────────────

#[test]
fn profiles_renders_active_profile_marker() {
    let model = model_connected();
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::profiles::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Profiles"), "should show Profiles title");
    assert!(out.contains("Default"), "should show profile name");
    assert!(out.contains("Streaming"), "should show all profiles");
    assert!(out.contains("▶"), "should show active marker");
}

#[test]
fn profiles_renders_empty_list_without_panic() {
    let mut model = model_connected();
    if let Some(ref mut snap) = model.snapshot {
        snap.profiles.clear();
        snap.current_profile = None;
    }
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::profiles::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Profiles"), "should still render block title");
}

// ── logs widget ──────────────────────────────────────────────────────────────

#[test]
fn logs_renders_recent_entries() {
    let model = model_connected();
    let mut t = term(60, 8);
    t.draw(|f| {
        widgets::logs::render(f, Rect::new(0, 0, 60, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Logs"), "should show Logs title");
    assert!(out.contains("INFO"), "should show level label");
    assert!(out.contains("server started"), "should show log message");
}

#[test]
fn logs_renders_empty_without_panic() {
    let mut model = model_connected();
    model.logs.clear();
    let mut t = term(60, 8);
    t.draw(|f| {
        widgets::logs::render(f, Rect::new(0, 0, 60, 8), &model);
    })
    .unwrap();
}

#[test]
fn logs_handles_small_terminal_height() {
    let model = model_connected();
    let mut t = term(60, 3);
    // Must not panic on very small area.
    t.draw(|f| {
        widgets::logs::render(f, Rect::new(0, 0, 60, 3), &model);
    })
    .unwrap();
}

// ── command palette widget ───────────────────────────────────────────────────

#[test]
fn command_palette_renders_help_text_when_inactive() {
    let model = model_connected();
    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::command_palette::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("Command Palette"),
        "should show palette title; got: {out}"
    );
}

#[test]
fn command_palette_renders_prompt_when_active() {
    let mut model = model_connected();
    model.command_palette.active = true;
    model.command_palette.input = "/scene main".into();
    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::command_palette::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains(">") || out.contains("scene"),
        "should show prompt or input; got: {out}"
    );
}

#[test]
fn command_palette_renders_completion_chips_in_five_line_area() {
    let mut model = model_connected();
    model.command_palette.active = true;
    model.command_palette.input = "/scene".into();
    model.command_palette.completions = vec!["/scene main".into(), "/scene cam".into()];
    model.command_palette.completion_idx = Some(1);

    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::command_palette::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();

    let out = buf_string(&t);
    assert!(
        out.contains("[/scene main]"),
        "should show bracketed completion candidate; got: {out}"
    );
    assert!(
        out.contains("[/scene cam]"),
        "should show selected bracketed completion candidate; got: {out}"
    );
}

#[test]
fn command_palette_renders_empty_completion_hint_in_five_line_area() {
    let mut model = model_connected();
    model.command_palette.active = true;
    model.command_palette.input = "/unknown".into();
    model.command_palette.completions.clear();
    model.command_palette.completion_idx = None;

    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::command_palette::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();

    let out = buf_string(&t);
    assert!(
        out.contains("no completions"),
        "should show empty completion hint; got: {out}"
    );
}

#[test]
fn command_palette_renders_last_result() {
    let mut model = model_connected();
    model.set_last_result("scene set: Main");
    // Let the typewriter reveal animation finish before asserting the
    // final rendered text.
    for _ in 0..20 {
        model.anim.tick();
    }
    let mut t = term(80, 5);
    t.draw(|f| {
        widgets::command_palette::render(f, Rect::new(0, 0, 80, 5), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("scene set"),
        "should show last result; got: {out}"
    );
}

// ── splash widget ───────────────────────────────────────────────────────────

#[test]
fn splash_renders_wordmark_tagline_and_progress_bar() {
    let mut t = term(60, 12);
    t.draw(|f| {
        widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 0, 40);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("O B S"),
        "should show the wordmark; got: {out}"
    );
    assert!(
        out.contains("OBS Studio Controller"),
        "should show the tagline; got: {out}"
    );
    assert!(out.contains('['), "should show a progress bar; got: {out}");
}

#[test]
fn splash_progress_bar_fills_as_frames_advance() {
    let mut t = term(60, 12);
    t.draw(|f| {
        widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 40, 40);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("████████████████████████"),
        "bar should be fully filled at the last frame; got: {out}"
    );
}

#[test]
fn splash_survives_minimum_terminal_size() {
    let mut t = term(20, 4);
    t.draw(|f| {
        widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 5, 40);
    })
    .unwrap();
}

// ── settings widget ─────────────────────────────────────────────────────────

#[test]
fn settings_renders_theme_list_and_preview() {
    let model = model_connected();
    let mut t = term(100, 20);
    t.draw(|f| {
        widgets::settings::render(f, Rect::new(0, 0, 100, 20), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Settings"), "should show settings title");
    assert!(out.contains("Themes"), "should show theme list title");
    assert!(out.contains("Claude"), "should list the claude preset");
    assert!(out.contains("Preview"), "should show a live preview panel");
}

#[test]
fn settings_survives_minimum_terminal_size() {
    let model = model_connected();
    let mut t = term(20, 6);
    t.draw(|f| {
        widgets::settings::render(f, Rect::new(0, 0, 20, 6), &model);
    })
    .unwrap();
}

// ── small terminal edge case ─────────────────────────────────────────────────

#[test]
fn all_widgets_survive_minimum_terminal_size() {
    let model = model_connected();
    let mut t = term(20, 4);
    t.draw(|f| {
        let area = f.area();
        widgets::header::render(f, area, &model);
    })
    .unwrap();

    let mut t2 = term(20, 4);
    t2.draw(|f| {
        let area = f.area();
        widgets::scenes::render(f, area, &model);
    })
    .unwrap();

    let mut t3 = term(20, 4);
    t3.draw(|f| {
        let area = f.area();
        widgets::audio::render(f, area, &model);
    })
    .unwrap();
}
