use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let height = area.height.saturating_sub(2) as usize;
    let skip = model.logs.len().saturating_sub(height);

    let items: Vec<ListItem> = model
        .logs
        .iter()
        .skip(skip)
        .map(|line| {
            let style = if line.contains("ERROR") || line.contains("error") {
                Style::default().fg(Color::Red)
            } else if line.contains("WARN") || line.contains("warn") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(Line::styled(line.clone(), style))
        })
        .collect();

    let block = Block::default().borders(Borders::ALL).title(" Logs ");
    f.render_widget(List::new(items).block(block), area);
}
