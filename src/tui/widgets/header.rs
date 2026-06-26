use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let daemon_status = if model.connected_to_daemon {
        Span::styled("daemon: connected", Style::default().fg(Color::Green))
    } else {
        Span::styled("daemon: disconnected", Style::default().fg(Color::Red))
    };

    let obs_status = if model.obs_connected() {
        let ver = model
            .snapshot
            .as_ref()
            .and_then(|s| s.obs_studio_version.as_deref())
            .unwrap_or("?");
        Span::styled(
            format!("OBS: connected (v{ver})"),
            Style::default().fg(Color::Green),
        )
    } else {
        Span::styled("OBS: disconnected", Style::default().fg(Color::Yellow))
    };

    let scene_span = if let Some(scene) = model.current_scene() {
        Span::raw(format!("  scene: {scene}"))
    } else {
        Span::raw("")
    };

    let streaming = model.snapshot.as_ref().map(|s| s.streaming).unwrap_or(false);
    let recording = model.snapshot.as_ref().map(|s| s.recording).unwrap_or(false);

    let stream_span = if streaming {
        Span::styled("  ● LIVE", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("  ○ LIVE:off", Style::default().fg(Color::DarkGray))
    };
    let rec_span = if recording {
        Span::styled("  ⏺ REC", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("  ○ REC:off", Style::default().fg(Color::DarkGray))
    };

    let line = Line::from(vec![
        Span::styled(
            "obsctl",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        daemon_status,
        Span::raw("  "),
        obs_status,
        scene_span,
        stream_span,
        rec_span,
    ]);

    let block = Block::default().borders(Borders::ALL).title(" obsctl-rs ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(line), inner);
}
