use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Command Palette ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(result) = &model.last_result {
        let style = if result.starts_with("error") {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::styled(result.clone(), style));
    } else {
        lines.push(Line::raw(""));
    }

    let prompt_line = if model.command_palette.active {
        Line::from(vec![
            Span::styled(
                "> ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(model.command_palette.input.clone()),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ])
    } else {
        Line::from(vec![Span::styled(
            " / or : to open command palette  q to quit  r reload  D dump",
            Style::default().fg(Color::DarkGray),
        )])
    };
    lines.push(prompt_line);

    f.render_widget(Paragraph::new(lines), inner);
}
