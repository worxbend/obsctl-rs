use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::tui::{model::TuiModel, widgets::chrome};

const MAX_VISIBLE_COMPLETIONS: usize = 8;
/// Reveal speed of the last-result typewriter animation, in characters per
/// render tick. Fast enough to feel snappy rather than sluggish.
const RESULT_REVEAL_CHARS_PER_TICK: usize = 3;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let block = chrome::panel(
        model.symbol("⌘", ">"),
        "Command Palette",
        if model.command_palette.active {
            model.symbol(
                "type command  ↵ run  ⇥ complete  esc close",
                "type command  Enter run  Tab complete  Esc close",
            )
        } else {
            ": open  <space> which-key  q quit"
        },
        model.command_palette.completions.len(),
        model.command_palette.active,
        model,
    );
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
                Span::styled(format!("{}  {revealed}", model.symbol("✉", "msg:")), style),
                Span::styled(
                    if model.advanced_ui { "▌" } else { "_" },
                    Style::default().fg(theme.accent),
                ),
            ]));
        } else {
            lines.push(Line::styled(
                format!("{}  {revealed}", model.symbol("✉", "msg:")),
                style,
            ));
        }
    } else {
        lines.push(Line::raw(""));
    }

    let prompt_line = if model.command_palette.active {
        Line::from(vec![
            Span::styled(
                format!("{}  ", model.symbol("❯", ">")),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(model.command_palette.input.clone()),
            Span::styled(
                if model.advanced_ui { "█" } else { "_" },
                Style::default().fg(theme.accent),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            if model.advanced_ui {
                "  ◈  : command  │  <space> which-key  │  gg/G jump  │  ⇥ pane  │  q quit"
            } else {
                "  >  : command  |  <space> which-key  |  gg/G jump  |  Tab pane  |  q quit"
            },
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
