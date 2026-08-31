use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use rust_i18n::t;

use crate::tui::{model::TuiModel, widgets::chrome};

const MAX_VISIBLE_COMPLETIONS: usize = 8;
/// Reveal speed of the last-result typewriter animation, in characters per
/// render tick. Fast enough to feel snappy rather than sluggish.
const RESULT_REVEAL_CHARS_PER_TICK: usize = 3;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let title = t!("tui.panels.palette.title");
    let hint = if model.command_palette.active {
        chrome::phrase(
            model,
            "tui.panels.palette.hint_typing",
            "tui.panels.palette.hint_typing_ascii",
        )
    } else {
        t!("tui.panels.palette.hint_idle").to_string()
    };
    let block = chrome::panel(
        model.symbol("⌘", ">"),
        &title,
        &hint,
        model.command_palette.completions.len(),
        model.command_palette.active,
        model,
    );
    let Some(inner) = chrome::frame(f, area, block) else {
        return;
    };

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
                    chrome::glyph(model, "▌", "_"),
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
                chrome::glyph(model, "█", "_"),
                Style::default().fg(theme.accent),
            ),
        ])
    } else {
        Line::from(vec![Span::styled(
            chrome::phrase(
                model,
                "tui.panels.palette.keys",
                "tui.panels.palette.keys_ascii",
            ),
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
        // Dimmed on top of the shared muted colour: this line sits under a
        // live prompt, so it should read as fainter than an empty panel does.
        return chrome::placeholder_line(
            model,
            t!("tui.panels.palette.no_completions").into_owned(),
        )
        .patch_style(Style::default().add_modifier(Modifier::DIM));
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
