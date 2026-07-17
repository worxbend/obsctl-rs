use ratatui::{
    style::{Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders},
};

use crate::tui::{anim, model::TuiModel};

pub const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Shared panel chrome: rounded inactive borders, a heavier focused frame,
/// animated gradient headings, a count badge, and a dim keyboard hint.
pub fn panel<'a>(
    icon: &'a str,
    label: &'a str,
    hint: &'a str,
    count: usize,
    focused: bool,
    model: &TuiModel,
) -> Block<'a> {
    let theme = model.theme;
    let frame = model.anim.frame;
    // Keep a full visual gutter after wide emoji glyphs. Some terminals render
    // emoji in two cells, making a single following space appear collapsed.
    let heading = format!(" {icon}  {label} ");
    let mut title_spans = if model.advanced_ui {
        anim::gradient_line(&heading, theme.accent, theme.accent_alt, frame, true).spans
    } else {
        vec![Span::styled(
            heading,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )]
    };
    title_spans.push(Span::styled(
        format!(" {count:02} "),
        Style::default()
            .fg(theme.highlight_fg)
            .bg(theme.highlight_bg)
            .add_modifier(Modifier::BOLD),
    ));
    if !hint.is_empty() {
        title_spans.push(Span::styled(
            format!("  {hint} "),
            Style::default().fg(theme.muted),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(if focused && model.advanced_ui {
            anim::blend(theme.border_focus, theme.accent_alt, 0.25)
        } else if focused {
            theme.border_focus
        } else {
            theme.border
        }))
        .title(Line::from(title_spans));
    if model.advanced_ui {
        block
    } else {
        block.border_set(ASCII_BORDER)
    }
}

pub fn status_dot(active: bool, fancy: bool) -> &'static str {
    match (active, fancy) {
        (true, true) => "●",
        (false, true) => "○",
        (true, false) => "*",
        (false, false) => "-",
    }
}
