use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::tui::{keymap, model::TuiModel, widgets::chrome};

/// Columns one `key → label` cell occupies, including its gutter.
const CELL_WIDTH: u16 = 26;
/// Widest the popup grows, however much room the terminal has.
const MAX_WIDTH: u16 = 78;

/// AstroNvim-style which-key popup: whenever a key sequence is half-typed,
/// show what the next keystroke can be. Rendered last so it floats over the
/// dashboard, anchored above the command bar.
///
/// Draws nothing when no sequence is pending, or when the terminal is too
/// small for the popup to say anything useful.
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let Some((title, entries)) = keymap::menu(model.pending) else {
        return;
    };
    if area.width < CELL_WIDTH + 4 || area.height < 6 {
        return;
    }

    let theme = model.theme;
    let columns = ((area.width.min(MAX_WIDTH).saturating_sub(2)) / CELL_WIDTH).max(1) as usize;
    let rows = entries.len().div_ceil(columns);

    let lines: Vec<Line> = (0..rows)
        .map(|row| {
            let mut spans = Vec::new();
            for column in 0..columns {
                let Some(e) = entries.get(row * columns + column) else {
                    break;
                };
                spans.push(Span::styled(
                    format!(" {:>3} ", e.key),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    model.symbol("→ ", "> "),
                    Style::default().fg(theme.muted),
                ));
                // which-key marks a prefix that opens another menu with a
                // leading `+`; keep that vocabulary so muscle memory carries.
                let (label, style) = if e.group {
                    (
                        format!("+{:<width$}", e.label, width = 15),
                        Style::default().fg(theme.accent_alt),
                    )
                } else {
                    (
                        format!("{:<width$}", e.label, width = 16),
                        Style::default().fg(theme.fg),
                    )
                };
                spans.push(Span::styled(label, style));
            }
            Line::from(spans)
        })
        .collect();

    let width = (columns as u16 * CELL_WIDTH + 2).min(area.width);
    let height = (rows as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        // Sit just above the command bar, which owns the bottom four rows.
        y: area.y + area.height.saturating_sub(height + 4),
        width,
        height,
    };

    let mut heading = vec![Span::styled(
        format!(" {title} "),
        Style::default()
            .fg(theme.highlight_fg)
            .bg(theme.highlight_bg)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(count) = model.pending_count {
        heading.push(Span::styled(
            format!(" {count} "),
            Style::default().fg(theme.warning),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border_focus))
        .style(Style::default().bg(theme.bg))
        .title(Line::from(heading));
    let block = if model.advanced_ui {
        block
    } else {
        block.border_set(chrome::ASCII_BORDER)
    };

    // The popup overlays live panels, so clear what is underneath first —
    // otherwise the dashboard bleeds through the gaps between cells.
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(lines).block(block), popup);
}
