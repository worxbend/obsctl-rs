use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::service::systemd_user_service::SYSTEMCTL_ENABLE_HINT;
use crate::tui::model::TuiModel;

pub fn render_unavailable(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let err = model
        .last_result
        .as_deref()
        .unwrap_or("Could not connect to obsctl daemon.");

    let lines = vec![
        Line::styled(
            "obsctl server is not running",
            Style::default().fg(theme.danger),
        ),
        Line::raw(""),
        Line::styled(err, Style::default().fg(theme.warning)),
        Line::raw(""),
        Line::raw("Start the daemon with:"),
        Line::styled(
            "  obsctl server --headless",
            Style::default().fg(theme.info),
        ),
        Line::raw("Or install the service:"),
        Line::styled("  obsctl service install", Style::default().fg(theme.info)),
        Line::styled(
            format!("  {SYSTEMCTL_ENABLE_HINT}"),
            Style::default().fg(theme.info),
        ),
        Line::raw(""),
        Line::styled(
            "Press R to retry, q to quit.",
            Style::default().fg(theme.muted),
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" obsctl — daemon unavailable ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let lines: Vec<Line> = if !model.connected_to_daemon {
        vec![
            Line::styled(
                "Not connected to obsctl daemon.",
                Style::default().fg(theme.danger),
            ),
            Line::raw("Press R to retry, q to quit."),
        ]
    } else if let Some(snap) = &model.snapshot {
        let mut v = vec![Line::from(format!(
            "OBS: {}",
            if snap.connected {
                "connected"
            } else {
                "disconnected"
            }
        ))];
        if let Some(e) = &snap.last_error {
            v.push(Line::styled(
                format!("last error: {e}"),
                Style::default().fg(theme.warning),
            ));
        }
        v
    } else {
        vec![Line::raw("Waiting for state...")]
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Connection ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}
