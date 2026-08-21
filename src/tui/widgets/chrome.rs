use ratatui::{
    style::{Color, Modifier, Style},
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

/// A plain bordered box in the theme's chrome, for the parts of the UI that
/// are not selectable list panels — the header, the status pane, the live bar,
/// the settings view, the connection notice.
///
/// Every one of them built the same `Block` by hand and then remembered to
/// swap in the ASCII border set when the model is in no-icon mode. Forgetting
/// that last step is invisible in a normal terminal and only shows up as
/// mojibake for the users who asked for ASCII output, so it happens here
/// instead of at seven call sites.
pub fn bordered<'a>(
    model: &TuiModel,
    border_type: BorderType,
    border_color: Color,
    title: impl Into<Line<'a>>,
) -> Block<'a> {
    ascii_aware(
        Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color))
            .title(title),
        model,
    )
}

/// Swap a block's box-drawing border for the ASCII one when the model is in
/// no-icon mode. Both [`bordered`] and [`panel`] apply this already; call it
/// directly only for a block built some other way.
pub fn ascii_aware<'a>(block: Block<'a>, model: &TuiModel) -> Block<'a> {
    if model.advanced_ui {
        block
    } else {
        block.border_set(ASCII_BORDER)
    }
}

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
    ascii_aware(block, model)
}

pub fn status_dot(active: bool, fancy: bool) -> &'static str {
    match (active, fancy) {
        (true, true) => "●",
        (false, true) => "○",
        (true, false) => "*",
        (false, false) => "-",
    }
}

/// Elapsed stream/record time as `mm:ss`, widening to `hh:mm:ss` only once
/// the session passes an hour so short sessions stay narrow. `None` (OBS has
/// not reported a duration yet) renders as a placeholder of the same width.
///
/// Shared by the live bar and the top-right status pane so both spell a
/// running broadcast the same way.
pub fn format_duration(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return "--:--".to_string();
    };
    let total_secs = ms / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_handles_none() {
        assert_eq!(format_duration(None), "--:--");
    }

    #[test]
    fn format_duration_formats_minutes_and_seconds() {
        assert_eq!(format_duration(Some(65_000)), "01:05");
    }

    #[test]
    fn format_duration_formats_hours_when_long() {
        assert_eq!(format_duration(Some(3_661_000)), "01:01:01");
    }
}
