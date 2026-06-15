use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let lines: Vec<Line> = if !model.connected_to_daemon {
        vec![
            Line::styled(
                "Not connected to obsctl daemon.",
                Style::default().fg(Color::Red),
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
                Style::default().fg(Color::Yellow),
            ));
        }
        v
    } else {
        vec![Line::raw("Waiting for state...")]
    };

    let block = Block::default().borders(Borders::ALL).title(" Connection ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(lines), inner);
}
