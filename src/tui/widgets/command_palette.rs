use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::model::TuiModel;

const MAX_VISIBLE_COMPLETIONS: usize = 8;
/// Reveal speed of the last-result typewriter animation, in characters per
/// render tick. Fast enough to feel snappy rather than sluggish.
const RESULT_REVEAL_CHARS_PER_TICK: usize = 3;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Command Palette ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(full) = &model.last_result {
        let style = if full.starts_with("error") {
            Style::default().fg(theme.danger)
        } else {
            Style::default().fg(theme.success)
        };
        let revealed = model
            .revealed_last_result(RESULT_REVEAL_CHARS_PER_TICK)
            .unwrap_or("");
        if revealed.len() < full.len() {
            lines.push(Line::from(vec![
                Span::styled(revealed.to_string(), style),
                Span::styled("▌", Style::default().fg(theme.accent)),
            ]));
        } else {
            lines.push(Line::styled(revealed.to_string(), style));
        }
    } else {
        lines.push(Line::raw(""));
    }

    let prompt_line = if model.command_palette.active {
        Line::from(vec![
            Span::styled(
                "> ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(model.command_palette.input.clone()),
            Span::styled("█", Style::default().fg(theme.accent)),
        ])
    } else {
        Line::from(vec![Span::styled(
            " / or : palette  F2/Ctrl-T themes  q quit  r reload  D dump",
            Style::default().fg(theme.muted),
        )])
    };
    lines.push(prompt_line);

    if model.command_palette.active {
        lines.push(completion_line(model));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn completion_line(model: &TuiModel) -> Line<'static> {
    let theme = model.theme;
    let completions = &model.command_palette.completions;
    if completions.is_empty() {
        return Line::from(Span::styled(
            "  no completions",
            Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
        ));
    }

    let mut spans = vec![Span::raw("  ")];
    for (i, completion) in completions.iter().take(MAX_VISIBLE_COMPLETIONS).enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let style = if Some(i) == model.command_palette.completion_idx {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        spans.push(Span::styled(format!("[{completion}]"), style));
    }

    Line::from(spans)
}
