use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::model::TuiModel;

const MAX_VISIBLE_COMPLETIONS: usize = 8;

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

    if model.command_palette.active {
        lines.push(completion_line(model));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn completion_line(model: &TuiModel) -> Line<'static> {
    let completions = &model.command_palette.completions;
    if completions.is_empty() {
        return Line::from(Span::styled(
            "  no completions",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::DIM),
        ));
    }

    let mut spans = vec![Span::raw("  ")];
    for (i, completion) in completions.iter().take(MAX_VISIBLE_COMPLETIONS).enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if Some(i) == model.command_palette.completion_idx {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("[{completion}]"), style));
    }

    Line::from(spans)
}
