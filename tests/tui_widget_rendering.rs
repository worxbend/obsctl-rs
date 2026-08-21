// TUI widget rendering tests using Ratatui's TestBackend.
// These tests verify that widgets render expected text without panicking
// across connected, disconnected, empty, and error states.

use obsctl_rs::{
    ipc::protocol::LogLevel,
    obs::state::{AudioState, ObsSnapshot, ObsStats, SceneState},
    tui::{
        keymap::Pending,
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
        scene_collections: vec!["Podcast".into(), "Gaming".into()],
        current_scene_collection: Some("Podcast".into()),
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
    model.set_snapshot(snap_connected());
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
fn live_bar_renders_idle_badge_and_stats_placeholder() {
    let model = model_connected();
    let mut t = term(100, 3);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 3), &model, true);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("IDLE"),
        "nothing running should read as IDLE; got: {out}"
    );
    assert!(
        !out.contains("LIVE 0") && !out.contains("REC 0"),
        "idle must not show a running duration; got: {out}"
    );
    assert!(
        out.contains("waiting"),
        "should show stats placeholder when no stats yet; got: {out}"
    );
}

#[test]
fn live_bar_omits_its_badges_when_the_status_pane_is_showing() {
    let model = model_connected();
    let mut t = term(100, 3);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 3), &model, false);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        !out.contains("IDLE"),
        "state is owned by the status pane here, not the live bar; got: {out}"
    );
    assert!(
        out.contains("Main"),
        "scene should still render; got: {out}"
    );
}

#[test]
fn live_bar_renders_active_stream_and_record_with_stats() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
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
    });
    let mut t = term(100, 3);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 3), &model, true);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("LIVE"), "should show LIVE badge");
    assert!(out.contains("01:05"), "should show stream duration");
    assert!(out.contains("00:05"), "should show record duration");
    assert!(out.contains("4500"), "should show bitrate");
    assert!(out.contains("60.0"), "should show fps");
}

#[test]
fn live_bar_draws_braille_meters_for_cpu_and_memory() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.stats = Some(obsctl_rs::obs::state::ObsStats {
            cpu_usage_percent: 42.0,
            active_fps: 60.0,
            memory_usage_mb: 512.0,
            ..Default::default()
        });
    });
    // Two rows of inner height, so the meter row renders.
    let mut t = term(100, 4);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 4), &model, false);
    })
    .unwrap();
    let out = buf_string(&t);

    assert!(out.contains("CPU"), "CPU meter label; got: {out}");
    assert!(out.contains("42.0%"), "CPU value; got: {out}");
    assert!(out.contains("MEM"), "MEM meter label; got: {out}");
    assert!(out.contains("512MB"), "memory value; got: {out}");
    // The braille bar's body glyph — proof the meters are bars, not text.
    assert!(
        out.contains('\u{28FF}'),
        "expected braille meter cells; got: {out}"
    );
}

#[test]
fn live_bar_meters_fall_back_to_ascii_in_simplified_mode() {
    let mut model = model_connected();
    model.advanced_ui = false;
    model.update_snapshot(|snap| {
        snap.stats = Some(obsctl_rs::obs::state::ObsStats {
            cpu_usage_percent: 50.0,
            active_fps: 60.0,
            memory_usage_mb: 128.0,
            ..Default::default()
        });
    });
    let mut t = term(100, 4);
    t.draw(|f| {
        widgets::live_bar::render(f, Rect::new(0, 0, 100, 4), &model, false);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.is_ascii(), "simplified meters must stay ASCII: {out}");
    assert!(
        out.contains("#####-----"),
        "CPU at 50% should be a half-filled ASCII meter; got: {out}"
    );
}

// ── status pane ─────────────────────────────────────────────────────────────

/// The pane spells its state in a 3-row block font; assert on the top row of
/// that art so the test fails if the word or the font stops rendering.
fn block_top_row(word: &str) -> String {
    obsctl_rs::tui::spinner::block_word(word, '█')[0].clone()
}

#[test]
fn status_pane_shows_idle_when_nothing_is_running() {
    let model = model_connected();
    let mut t = term(32, 8);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 32, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains(&block_top_row("IDLE")),
        "idle state should render IDLE in block letters; got: {out}"
    );
    assert!(out.contains("standing by"), "idle hint; got: {out}");
    assert!(
        !out.contains(&block_top_row("LIVE")),
        "nothing is streaming; got: {out}"
    );
}

#[test]
fn status_pane_shows_live_and_rec_side_by_side_with_durations() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.streaming = true;
        snap.recording = true;
        snap.stream_duration_ms = Some(65_000);
        snap.record_duration_ms = Some(5_000);
    });
    let mut t = term(32, 8);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 32, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains(&block_top_row("LIVE")),
        "LIVE block letters; got: {out}"
    );
    assert!(
        out.contains(&block_top_row("REC")),
        "REC block letters; got: {out}"
    );
    assert!(out.contains("01:05"), "stream duration; got: {out}");
    assert!(out.contains("00:05"), "record duration; got: {out}");
    assert!(
        !out.contains(&block_top_row("IDLE")),
        "not idle while on air; got: {out}"
    );
}

#[test]
fn status_pane_shows_rec_alone_while_only_recording() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.recording = true;
        snap.record_duration_ms = Some(3_600_000);
    });
    let mut t = term(32, 8);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 32, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains(&block_top_row("REC")), "REC art; got: {out}");
    assert!(
        !out.contains(&block_top_row("LIVE")),
        "not streaming; got: {out}"
    );
    assert!(out.contains("01:00:00"), "hour-long record; got: {out}");
}

#[test]
fn status_pane_spinner_distinguishes_live_from_idle_at_the_same_tick() {
    use obsctl_rs::tui::spinner::{self, BroadcastState};

    let idle = spinner::frame(BroadcastState::Idle, true, 7);
    let live = spinner::frame(BroadcastState::Live, true, 7);
    let rec = spinner::frame(BroadcastState::Rec, true, 7);

    let mut model = model_connected();
    let mut t = term(32, 8);
    t.draw(|f| widgets::status::render(f, Rect::new(0, 0, 32, 8), &model))
        .unwrap();
    let idle_out = buf_string(&t);

    model.update_snapshot(|snap| {
        snap.streaming = true;
    });
    let mut t = term(32, 8);
    t.draw(|f| widgets::status::render(f, Rect::new(0, 0, 32, 8), &model))
        .unwrap();
    let live_out = buf_string(&t);

    assert_ne!(
        idle, live,
        "idle and live must not share a spinner frame at tick 7"
    );
    assert_ne!(live, rec);
    assert_ne!(idle_out, live_out, "the pane should look different on air");
}

#[test]
fn status_pane_degrades_to_a_single_line_when_too_small_for_block_letters() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.streaming = true;
        snap.stream_duration_ms = Some(65_000);
    });
    // Three rows total leaves one inner row — no room for the 3-row font.
    let mut t = term(24, 3);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 24, 3), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("LIVE"),
        "compact form still names the state; got: {out}"
    );
    assert!(
        !out.contains(&block_top_row("LIVE")),
        "block art must not be attempted at this size; got: {out}"
    );
}

#[test]
fn status_pane_renders_without_a_snapshot() {
    // No OBS data at all (daemon up, OBS not connected yet) must still paint
    // a state rather than panicking on a missing snapshot.
    let mut model = TuiModel::default();
    model.connected_to_daemon = true;
    let mut t = term(32, 8);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 32, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains(&block_top_row("IDLE")),
        "no snapshot reads as idle; got: {out}"
    );
    assert!(out.contains("--:--") || out.contains("standing by"));
}

#[test]
fn status_pane_is_ascii_in_simplified_mode() {
    let mut model = model_connected();
    model.advanced_ui = false;
    model.show_icons = true;
    model.update_snapshot(|snap| {
        snap.streaming = true;
        snap.recording = true;
        snap.stream_duration_ms = Some(1_000);
        snap.record_duration_ms = Some(1_000);
    });
    let mut t = term(32, 8);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 32, 8), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.is_ascii(),
        "simplified status pane must be ASCII: {out}"
    );
    assert!(
        out.contains(&obsctl_rs::tui::spinner::block_word("LIVE", '#')[0]),
        "ASCII block letters; got: {out}"
    );
}

#[test]
fn status_pane_survives_a_degenerate_area() {
    let model = model_connected();
    let mut t = term(4, 2);
    t.draw(|f| {
        widgets::status::render(f, Rect::new(0, 0, 4, 2), &model);
    })
    .unwrap();
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
    model.set_snapshot(snap_disconnected_with_error());
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
    model.clear_snapshot();
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

/// Foreground colors on the row the cursor is on, left to right.
fn row_foregrounds(terminal: &Terminal<TestBackend>, y: u16) -> Vec<Option<ratatui::style::Color>> {
    let buf = terminal.backend().buffer().clone();
    (0..buf.area().width)
        .map(|x| buf.cell((x, y)).unwrap().style().fg)
        .collect()
}

#[test]
fn selected_scene_is_tinted_rather_than_painted_over() {
    use obsctl_rs::tui::model::FocusPanel;

    let mut model = model_connected();
    model.focus = FocusPanel::Scenes;
    model.set_panel_cursor(FocusPanel::Scenes, 0);
    let theme = model.theme;

    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::scenes::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();

    // Row 1 is the first list entry (row 0 is the panel border).
    let buf = t.backend().buffer().clone();
    let selected_bg = buf.cell((2, 1)).unwrap().style().bg;
    let expected = theme.selection_style(true).bg;

    assert_eq!(
        selected_bg, expected,
        "selected row should carry the blended tint"
    );
    assert_ne!(
        selected_bg,
        Some(theme.highlight_bg),
        "selected row must not be the solid highlight bar"
    );

    // The whole point of tinting: the row keeps its own span colors instead
    // of every cell being repainted to one foreground.
    let foregrounds: std::collections::HashSet<_> = row_foregrounds(&t, 1)
        .into_iter()
        .flatten()
        .filter(|c| *c != theme.border)
        .collect();
    assert!(
        foregrounds.len() > 1,
        "selected row was flattened to a single foreground: {foregrounds:?}"
    );
    assert!(
        foregrounds.contains(&theme.success),
        "the active-scene marker should keep its color under the selection; got {foregrounds:?}"
    );
}

#[test]
fn unfocused_scene_selection_is_fainter_than_focused() {
    use obsctl_rs::tui::model::FocusPanel;

    let mut model = model_connected();
    model.set_panel_cursor(FocusPanel::Scenes, 0);

    let render_bg = |model: &TuiModel| {
        let mut t = term(60, 10);
        t.draw(|f| widgets::scenes::render(f, Rect::new(0, 0, 60, 10), model))
            .unwrap();
        t.backend()
            .buffer()
            .clone()
            .cell((2, 1))
            .unwrap()
            .style()
            .bg
    };

    model.focus = FocusPanel::Scenes;
    let focused = render_bg(&model);
    model.focus = FocusPanel::Audio;
    let unfocused = render_bg(&model);

    assert_ne!(
        focused, unfocused,
        "losing focus should soften the selection"
    );
    assert_eq!(focused, model.theme.selection_style(true).bg);
}

#[test]
fn scenes_renders_empty_list_without_panic() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.scenes.clear();
    });
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
    model.update_snapshot(|snap| {
        snap.scenes = vec![SceneState {
            name: "A".repeat(200),
            active: true,
            ..Default::default()
        }];
    });
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

/// Draw the audio matrix on its own and return the buffer as lines, so a
/// test can talk about columns as well as text.
fn audio_lines(model: &TuiModel, width: u16, height: u16) -> Vec<String> {
    let mut t = term(width, height);
    t.draw(|f| {
        widgets::audio::render(f, Rect::new(0, 0, width, height), model);
    })
    .unwrap();
    buf_string(&t).lines().map(str::to_string).collect()
}

#[test]
fn audio_renders_mute_and_volume() {
    let model = model_connected();
    let out = audio_lines(&model, 80, 12).join("\n");
    assert!(out.contains("Audio"), "should show Audio title");
    assert!(out.contains("Mic"), "should show audio input name");
    assert!(out.contains("80%"), "should show volume; got: {out}");
    assert!(
        out.contains("mute"),
        "a muted input should say so; got: {out}"
    );
}

#[test]
fn audio_stacks_each_input_into_its_own_bordered_vertical_strip() {
    let mut model = model_connected();
    model.record_meter_level("Mic".into(), 0.5);
    let lines = audio_lines(&model, 60, 14);

    // Both inputs are on the same row, side by side, rather than stacked.
    // Each strip carries its name on its own top border.
    let top = lines
        .iter()
        .find(|line| line.contains("Mic"))
        .expect("a row carrying the input names");
    assert!(
        top.contains("Desktop"),
        "inputs should sit side by side on one row; got: {top}"
    );
    assert!(
        top.find("Mic") < top.find("Desktop"),
        "strips keep snapshot order left to right; got: {top}"
    );

    // One box per input, with a blank column immediately before the second
    // box starts — the gap the strips are separated by.
    let cells: Vec<char> = top.chars().collect();
    let corners: Vec<usize> = cells
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '╭' || **c == '┏')
        .map(|(i, _)| i)
        .collect();
    assert_eq!(corners.len(), 2, "one border per input; got: {top}");
    assert_eq!(
        cells[corners[1] - 1],
        ' ',
        "strips should be separated by a blank column; got: {top}"
    );

    // Every meter is ruled with its own dB scale. The floor tick is the one
    // to count: it is drawn whatever step the scale settles on, and unlike a
    // bare "0" it cannot be matched by the panel's count badge or by "80%".
    let joined = lines.join("\n");
    assert_eq!(
        joined.matches("-60").count(),
        2,
        "each strip should rule its own dB scale down to the floor; got: {joined}"
    );
}

#[test]
fn audio_meters_line_up_across_strips() {
    // "Mic" has an alias and "Desktop" does not, but the extra row must be
    // spent on both strips so their meters stay level with each other.
    let mut model = model_connected();
    model.record_meter_level("Mic".into(), 0.5);
    model.record_meter_level("Desktop".into(), 0.1);
    let lines = audio_lines(&model, 60, 16);

    let scale_rows: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("-60"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        scale_rows.len(),
        1,
        "both -60 marks belong on the same row; got: {lines:#?}"
    );
    assert_eq!(
        lines[scale_rows[0]].matches("-60").count(),
        2,
        "each strip rules its own scale; got: {}",
        lines[scale_rows[0]]
    );
}

#[test]
fn audio_uses_ascii_symbols_when_icons_are_disabled() {
    let mut model = model_connected();
    model.show_icons = false;
    let out = audio_lines(&model, 80, 12).join("\n");
    assert!(!out.contains("🔊"));
    assert!(!out.contains("🔇"));
    assert!(
        out.contains("A 80%"),
        "unmuted input should keep its ASCII marker and volume; got: {out}"
    );
    assert!(
        out.contains("M mute"),
        "muted input should keep its ASCII marker; got: {out}"
    );
}

#[test]
fn audio_renders_empty_list_without_panic() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.audio_inputs.clear();
    });
    let out = audio_lines(&model, 60, 8).join("\n");
    assert!(out.contains("Audio"), "should still render block title");
    assert!(
        out.contains("no audio inputs"),
        "an empty matrix should say why it is empty; got: {out}"
    );
}

#[test]
fn audio_says_so_when_the_pane_is_too_narrow_for_a_strip() {
    let model = model_connected();
    let out = audio_lines(&model, 12, 8).join("\n");
    assert!(
        out.contains("too narrow"),
        "should explain the empty pane rather than just drawing a box; got: {out}"
    );
}

#[test]
fn audio_scrolls_sideways_to_keep_the_selected_strip_visible() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.audio_inputs = (0..6)
            .map(|i| AudioState {
                name: format!("In{i}"),
                muted: Some(false),
                volume_percent: Some(50),
                ..Default::default()
            })
            .collect();
    });
    model.clamp_cursors();

    // Only three strips fit in 38 columns of pane interior.
    let first = audio_lines(&model, 40, 12).join("\n");
    assert!(first.contains("In0") && first.contains("In2"));
    assert!(!first.contains("In5"), "In5 is off the right edge yet");

    use obsctl_rs::tui::model::FocusPanel;

    model.set_panel_cursor(FocusPanel::Audio, 5);
    let scrolled = audio_lines(&model, 40, 12).join("\n");
    assert!(
        scrolled.contains("In5"),
        "the selected strip must be on screen; got: {scrolled}"
    );
    assert!(
        !scrolled.contains("In0"),
        "the window should have scrolled past In0; got: {scrolled}"
    );
}

#[test]
fn audio_renders_long_name_without_panic() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.audio_inputs = vec![AudioState {
            name: "Z".repeat(200),
            muted: Some(false),
            volume_percent: Some(100),
            ..Default::default()
        }];
    });
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
    model.update_snapshot(|snap| {
        snap.profiles.clear();
        snap.current_profile = None;
    });
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::profiles::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Profiles"), "should still render block title");
}

// ── collections widget ──────────────────────────────────────────────────────

#[test]
fn collections_renders_active_collection_marker() {
    let model = model_connected();
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::collections::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Collections"), "should show Collections title");
    assert!(out.contains("Podcast"), "should show collection name");
    assert!(out.contains("Gaming"), "should show all collections");
    assert!(out.contains("▶"), "should show active marker");
}

#[test]
fn collections_renders_empty_list_without_panic() {
    let mut model = model_connected();
    model.update_snapshot(|snap| {
        snap.scene_collections.clear();
        snap.current_scene_collection = None;
    });
    let mut t = term(60, 10);
    t.draw(|f| {
        widgets::collections::render(f, Rect::new(0, 0, 60, 10), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("Collections"),
        "should still render block title"
    );
}

// ── full dashboard layout ────────────────────────────────────────────────────

#[test]
fn full_dashboard_renders_all_panels_with_new_layout() {
    use obsctl_rs::tui::layout;

    let model = model_connected();
    let mut t = term(100, 40);
    t.draw(|f| {
        let areas = layout::compute(f, model.streaming());
        widgets::header::render(f, areas.header, &model);
        widgets::live_bar::render(f, areas.live_bar, &model, areas.status.is_none());
        if let Some(status) = areas.status {
            widgets::status::render(f, status, &model);
        }
        widgets::scenes::render(f, areas.scenes, &model);
        widgets::audio::render(f, areas.audio, &model);
        widgets::profiles::render(f, areas.profiles, &model);
        widgets::collections::render(f, areas.collections, &model);
        widgets::logs::render(f, areas.logs, &model);
        widgets::command_palette::render(f, areas.palette, &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("Scenes"), "scenes panel should render");
    assert!(out.contains("Audio"), "audio panel should render");
    assert!(out.contains("Profiles"), "profiles panel should render");
    assert!(
        out.contains("Collections"),
        "collections panel should render"
    );
    assert!(out.contains("Main"), "scene name should render");
    assert!(out.contains("Mic"), "audio input name should render");
    assert!(out.contains("Streaming"), "profile name should render");
    assert!(out.contains("Gaming"), "collection name should render");
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
fn logs_scrolled_back_shows_older_entries_and_says_it_is_not_following() {
    let mut model = model_connected();
    model.logs.clear();
    for i in 0..20 {
        model.push_log(TuiLogEntry {
            level: LogLevel::Info,
            message: format!("entry-{i:02}"),
            target: None,
            timestamp: OffsetDateTime::UNIX_EPOCH,
        });
    }

    // Following the tail: newest visible, oldest scrolled off.
    let mut tail = term(60, 8);
    tail.draw(|f| {
        widgets::logs::render(f, Rect::new(0, 0, 60, 8), &model);
    })
    .unwrap();
    let tail_out = buf_string(&tail);
    assert!(tail_out.contains("entry-19"), "tail shows the newest entry");
    assert!(!tail_out.contains("entry-00"));

    // Scrolled back by six lines: the oldest entries come into view and the
    // pane stops advertising itself as a live feed.
    model.scroll_logs_up(6, 6);
    let mut scrolled = term(60, 8);
    scrolled
        .draw(|f| {
            widgets::logs::render(f, Rect::new(0, 0, 60, 8), &model);
        })
        .unwrap();
    let out = buf_string(&scrolled);
    assert!(out.contains("entry-08"), "scrolled window shows history");
    assert!(!out.contains("entry-19"), "newest entry is scrolled past");
    assert!(!out.contains("live daemon feed"));
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
        out.contains("Broadcast control, without breaking flow."),
        "should show the tagline; got: {out}"
    );
    assert!(
        out.contains("0%"),
        "should show progress percentage; got: {out}"
    );
    assert!(
        out.contains("SYNC"),
        "should show animated loader; got: {out}"
    );
    assert!(out.contains("LIVE"), "should show animated live identity");
}

#[test]
fn splash_renders_large_logo_and_layered_loaders() {
    let mut t = term(100, 20);
    t.draw(|f| {
        widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 12, 40);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("██████"), "should render large OBSCTL logo");
    assert!(
        out.contains("Scenes, audio, profiles, recording, and live telemetry"),
        "should describe the app"
    );
    assert!(out.contains("STUDIO LINK"), "should render the boot card");
    assert!(out.contains("LIVE"), "should render the live badge");
    assert!(out.contains("SIGNAL"), "should render slither loader");
    assert!(out.contains("LIQUID"), "should render liquid-wave loader");
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
fn splash_renders_the_preparing_band() {
    let mut t = term(100, 20);
    t.draw(|f| {
        widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 12, 40);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.contains("Preparing..."),
        "should label the preparing stage; got: {out}"
    );
    // The band is a field of braille dots (U+2800..U+28FF). Its content is
    // reseeded every render, so assert the character class, not the glyphs.
    let braille = out
        .chars()
        .filter(|c| ('\u{2800}'..='\u{28FF}').contains(c))
        .count();
    assert!(
        braille >= 20,
        "expected a braille shimmer band, found {braille} braille cells in: {out}"
    );
}

#[test]
fn splash_preparing_band_shimmers_between_renders() {
    let render_once = || {
        let mut t = term(100, 20);
        t.draw(|f| {
            widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 12, 40);
        })
        .unwrap();
        buf_string(&t)
    };
    // Same frame number, different noise — the band is what animates here.
    // 1-in-256^44 says this cannot collide by chance.
    assert_ne!(
        render_once(),
        render_once(),
        "the preparing band should reseed on every render"
    );
}

#[test]
fn splash_preparing_band_is_ascii_noise_in_simplified_mode() {
    let mut t = term(100, 20);
    t.draw(|f| {
        widgets::splash::render_with_appearance(
            f,
            obsctl_rs::tui::theme::Theme::default_theme(),
            12,
            40,
            false,
            false,
        );
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.is_ascii(), "ASCII splash must stay ASCII: {out}");
    assert!(out.contains("Preparing..."), "got: {out}");
}

#[test]
fn splash_survives_minimum_terminal_size() {
    let mut t = term(20, 4);
    t.draw(|f| {
        widgets::splash::render(f, obsctl_rs::tui::theme::Theme::default_theme(), 5, 40);
    })
    .unwrap();
}

#[test]
fn simplified_ui_renders_the_dashboard_and_splash_as_ascii_only() {
    let mut model = model_connected();
    model.advanced_ui = false;
    // Icons remain enabled in config by default; simplified mode must still
    // force every widget onto its ASCII fallback.
    model.show_icons = true;
    model.record_meter_level("Mic".into(), 0.42);

    let mut dashboard = term(120, 34);
    dashboard
        .draw(|f| {
            widgets::header::render(f, Rect::new(0, 0, 88, 4), &model);
            widgets::live_bar::render(f, Rect::new(0, 4, 88, 4), &model, false);
            widgets::status::render(f, Rect::new(88, 0, 32, 8), &model);
            widgets::scenes::render(f, Rect::new(0, 8, 40, 10), &model);
            widgets::audio::render(f, Rect::new(40, 8, 40, 10), &model);
            widgets::profiles::render(f, Rect::new(80, 8, 20, 10), &model);
            widgets::collections::render(f, Rect::new(100, 8, 20, 10), &model);
            widgets::logs::render(f, Rect::new(0, 18, 120, 8), &model);
            widgets::command_palette::render(f, Rect::new(0, 26, 120, 4), &model);
        })
        .unwrap();
    let dashboard_output = buf_string(&dashboard);
    assert!(
        dashboard_output.is_ascii(),
        "simplified dashboard emitted non-ASCII characters: {dashboard_output}"
    );

    let mut settings = term(100, 20);
    settings
        .draw(|f| {
            widgets::settings::render(f, f.area(), &model);
        })
        .unwrap();
    assert!(buf_string(&settings).is_ascii());

    let mut unavailable = term(100, 20);
    unavailable
        .draw(|f| widgets::connection::render_unavailable(f, f.area(), &model))
        .unwrap();
    let unavailable_output = buf_string(&unavailable);
    assert!(
        unavailable_output.is_ascii(),
        "simplified connection view emitted non-ASCII characters: {unavailable_output}"
    );

    let mut splash = term(100, 20);
    splash
        .draw(|f| {
            widgets::splash::render_with_appearance(f, model.theme, 12, 40, false, false);
        })
        .unwrap();
    let splash_output = buf_string(&splash);
    assert!(
        splash_output.is_ascii(),
        "simplified splash emitted non-ASCII characters: {splash_output}"
    );
    assert!(splash_output.contains("OBSCTL STARTUP"));
    assert!(splash_output.contains("LIVE"));
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
fn settings_scrolls_to_the_last_theme() {
    let mut model = model_connected();
    let last = obsctl_rs::tui::theme::ALL.len() - 1;
    model.settings_cursor = last;
    model.theme = obsctl_rs::tui::theme::ALL[last];

    let mut t = term(100, 20);
    t.draw(|f| {
        widgets::settings::render(f, Rect::new(0, 0, 100, 20), &model);
    })
    .unwrap();

    let out = buf_string(&t);
    assert!(out.contains("Mono (TTY-safe)"));
    assert!(out.contains("Preview: Mono (TTY-safe)"));
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

// ── which-key overlay ───────────────────────────────────────────────────────

#[test]
fn which_key_draws_nothing_until_a_sequence_is_pending() {
    let model = model_connected();
    let mut t = term(80, 24);
    t.draw(|f| {
        widgets::which_key::render(f, f.area(), &model);
    })
    .unwrap();
    assert!(
        buf_string(&t).trim().is_empty(),
        "no pending sequence should draw no popup"
    );
}

#[test]
fn which_key_lists_the_leader_menu_and_marks_groups() {
    let mut model = model_connected();
    model.pending = Pending::Leader;
    let mut t = term(90, 26);
    t.draw(|f| {
        widgets::which_key::render(f, f.area(), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("<leader>"), "popup names the pending sequence");
    assert!(out.contains("+find"), "groups are marked with a leading +");
    assert!(out.contains("+stream"));
    assert!(out.contains("quit"), "plain actions are listed too");
}

#[test]
fn which_key_shows_the_subgroup_after_a_second_key() {
    let mut model = model_connected();
    model.pending = Pending::LeaderGroup('c');
    let mut t = term(90, 26);
    t.draw(|f| {
        widgets::which_key::render(f, f.area(), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("<leader>c"));
    assert!(out.contains("reload config"));
    assert!(out.contains("validate config"));
}

#[test]
fn which_key_shows_a_typed_count_prefix() {
    let mut model = model_connected();
    model.pending = Pending::G;
    model.pending_count = Some(12);
    let mut t = term(90, 26);
    t.draw(|f| {
        widgets::which_key::render(f, f.area(), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(out.contains("12"), "the pending count is visible: {out}");
    assert!(out.contains("top of list"));
}

#[test]
fn which_key_uses_ascii_only_symbols_in_simplified_mode() {
    let mut model = model_connected();
    model.advanced_ui = false;
    model.show_icons = false;
    model.pending = Pending::Leader;
    let mut t = term(90, 26);
    t.draw(|f| {
        widgets::which_key::render(f, f.area(), &model);
    })
    .unwrap();
    let out = buf_string(&t);
    assert!(
        out.is_ascii(),
        "simplified which-key emitted non-ASCII characters: {out}"
    );
}

#[test]
fn which_key_stays_silent_on_a_terminal_too_small_to_hold_it() {
    let mut model = model_connected();
    model.pending = Pending::Leader;
    let mut t = term(20, 5);
    t.draw(|f| {
        widgets::which_key::render(f, f.area(), &model);
    })
    .unwrap();
    assert!(buf_string(&t).trim().is_empty());
}

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

// ── stats pane (stream health) ───────────────────────────────────────────────

fn snap_streaming(stats: ObsStats) -> ObsSnapshot {
    ObsSnapshot {
        streaming: true,
        stats: Some(stats),
        stream_bitrate_kbps: Some(6000.0),
        stream_duration_ms: Some(65_000),
        ..snap_connected()
    }
}

fn healthy_stats() -> ObsStats {
    ObsStats {
        cpu_usage_percent: 12.5,
        memory_usage_mb: 512.0,
        available_disk_space_mb: 100_000.0,
        active_fps: 60.0,
        average_frame_render_time_ms: 3.2,
        render_skipped_frames: 0,
        render_total_frames: 10_000,
        output_skipped_frames: 0,
        output_total_frames: 9_999,
    }
}

fn model_streaming(stats: ObsStats) -> TuiModel {
    let mut model = model_connected();
    model.set_snapshot(snap_streaming(stats));
    model.clamp_cursors();
    model.record_metric_sample();
    model
}

fn draw_stats(model: &TuiModel) -> String {
    let mut t = term(46, 7);
    t.draw(|f| {
        widgets::stats::render(f, Rect::new(0, 0, 46, 7), model);
    })
    .unwrap();
    buf_string(&t)
}

#[test]
fn stats_pane_reports_every_frame_metric_while_streaming() {
    let out = draw_stats(&model_streaming(healthy_stats()));

    assert!(
        out.contains("Stats"),
        "should render the pane title; got:\n{out}"
    );
    assert!(out.contains("FPS"), "should label active FPS");
    assert!(out.contains("60.0"), "should show active FPS value");
    assert!(out.contains("target 60"), "should show the FPS reference");
    assert!(out.contains("FRAME"), "should label frame render time");
    assert!(
        out.contains("3.20ms"),
        "should show average frame render time"
    );
    assert!(out.contains("budget"), "should show frame budget usage");
    assert!(out.contains("RENDER"), "should label render-skipped frames");
    assert!(out.contains("OUTPUT"), "should label output-skipped frames");
    assert!(out.contains("10.0k"), "should show render frame totals");
    assert!(
        out.contains("HEALTHY"),
        "a clean stream should read healthy"
    );
}

#[test]
fn stats_pane_falls_back_to_lifetime_counters_on_the_baseline_sample() {
    // The first sample of a stream *is* the baseline, so there are no
    // per-stream frames to divide by yet.
    let out = draw_stats(&model_streaming(healthy_stats()));
    assert!(
        out.contains("since launch"),
        "first sample should label counters as lifetime; got:\n{out}"
    );
    assert!(
        !out.contains("0/0"),
        "should never show an empty per-stream ratio; got:\n{out}"
    );
}

#[test]
fn stats_pane_reports_per_stream_drops_and_flags_a_degraded_stream() {
    let mut model = model_streaming(healthy_stats());
    // Second poll: FPS has sagged, frames are being skipped.
    model.update_snapshot(|snapshot| {
        let Some(stats) = snapshot.stats.as_mut() else {
            return;
        };
        stats.active_fps = 51.3;
        stats.average_frame_render_time_ms = 14.9;
        stats.render_total_frames = 10_600;
        stats.render_skipped_frames = 9;
        stats.output_total_frames = 10_580;
        stats.output_skipped_frames = 42;
    });
    model.record_metric_sample();

    let out = draw_stats(&model);
    assert!(
        out.contains("this stream"),
        "should switch to per-stream counters once measurable; got:\n{out}"
    );
    // 9 skipped of the 600 frames rendered since the stream started — not
    // 9 of OBS's 10 600 lifetime frames.
    assert!(
        out.contains("9/600"),
        "should subtract the per-stream baseline; got:\n{out}"
    );
    assert!(
        out.contains("1.50%"),
        "should show the per-stream render drop rate"
    );
    assert!(out.contains("51.3"), "should show the sagging FPS");
    assert!(
        out.contains("DROPPING"),
        "7% output drops should read as dropping; got:\n{out}"
    );
}

#[test]
fn stats_pane_waits_for_metrics_before_reporting() {
    let mut model = model_streaming(healthy_stats());
    model.update_snapshot(|snapshot| snapshot.stats = None);

    let out = draw_stats(&model);
    assert!(
        out.contains("waiting"),
        "should show a placeholder before the first poll; got:\n{out}"
    );
    assert!(!out.contains("FPS"), "should not show empty metric rows");
}

#[test]
fn stats_pane_uses_ascii_fallbacks_in_simple_mode() {
    let mut model = model_streaming(healthy_stats());
    model.advanced_ui = false;
    model.show_icons = false;

    let out = draw_stats(&model);
    assert!(
        out.contains("FPS"),
        "should still show metrics; got:\n{out}"
    );
    assert!(out.contains("##"), "budget bar should use ASCII blocks");
    assert!(
        !out.contains('█') && !out.contains('░') && !out.contains('╭'),
        "should not emit Unicode blocks or rounded borders; got:\n{out}"
    );
}

#[test]
fn stats_pane_survives_minimum_terminal_size() {
    let model = model_streaming(healthy_stats());
    let mut t = term(12, 3);
    t.draw(|f| {
        widgets::stats::render(f, Rect::new(0, 0, 12, 3), &model);
    })
    .unwrap();
}

#[test]
fn dashboard_shows_the_stats_pane_only_while_streaming() {
    use obsctl_rs::tui::layout;

    fn dashboard(model: &TuiModel) -> String {
        let mut t = term(120, 40);
        t.draw(|f| {
            let areas = layout::compute(f, model.streaming());
            widgets::logs::render(f, areas.logs, model);
            if let Some(stats) = areas.stats {
                widgets::stats::render(f, stats, model);
            }
        })
        .unwrap();
        buf_string(&t)
    }

    let idle = dashboard(&model_connected());
    assert!(idle.contains("Logs"), "logs should always render");
    assert!(
        !idle.contains("Stream Health"),
        "stats pane should stay hidden while idle; got:\n{idle}"
    );

    let live = dashboard(&model_streaming(healthy_stats()));
    assert!(
        live.contains("Logs"),
        "logs should keep its pane while streaming"
    );
    assert!(
        live.contains("Stream Health"),
        "stats pane should appear beside logs while streaming; got:\n{live}"
    );
}
