use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};

use crate::tui::{anim, model::TuiModel, theme, widgets::chrome};

/// Full-screen settings view — currently just the theme picker, styled
/// after btop's theme switcher: arrow keys live-preview a theme across the
/// whole UI, Enter confirms and persists it, Esc reverts to whatever was
/// active before opening this view.
/// Draws the settings view and returns the theme list's area together with the
/// scroll offset Ratatui settled on, for the mouse code to map a click to a
/// row. See [`crate::tui::mouse`].
#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) -> (Rect, usize) {
    let theme = model.theme;

    let title = if model.advanced_ui {
        anim::gradient_line(
            " ⚙ Settings // Appearance Lab // ↑↓/jk preview · Enter apply · Esc cancel ",
            theme.accent,
            theme.accent_alt,
            model.anim.frame,
            true,
        )
    } else {
        Line::styled(
            " Settings // Appearance // arrows preview | Enter apply | Esc cancel ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme.border_focus))
        .title(title);
    let outer = if model.advanced_ui {
        outer
    } else {
        outer.border_set(chrome::ASCII_BORDER)
    };
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    let offset = render_theme_list(f, sections[0], model);
    render_preview(f, sections[1], model);
    (sections[0], offset)
}

#[must_use]
fn render_theme_list(f: &mut Frame, area: Rect, model: &TuiModel) -> usize {
    let theme = model.theme;
    let items: Vec<ListItem> = theme::ALL
        .iter()
        .map(|t| {
            let cell = if model.advanced_ui { "██" } else { "##" };
            let swatch = Line::from(vec![
                Span::styled(cell, Style::default().fg(t.accent)),
                Span::styled(cell, Style::default().fg(t.success)),
                Span::styled(cell, Style::default().fg(t.warning)),
                Span::styled(cell, Style::default().fg(t.danger)),
                Span::raw("  "),
                Span::raw(t.label),
            ]);
            ListItem::new(swatch)
        })
        .collect();

    let block = chrome::bordered(
        model,
        BorderType::Rounded,
        theme.border,
        format!(" Themes // {:02} palettes ", theme::ALL.len()),
    );

    let highlight_style = Style::default()
        .bg(theme.highlight_bg)
        .fg(theme.highlight_fg)
        .add_modifier(Modifier::BOLD);

    let mut state = ListState::default();
    state.select(Some(model.settings_cursor));

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style),
        area,
        &mut state,
    );
    state.offset()
}

fn render_preview(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let block = chrome::bordered(
        model,
        BorderType::Rounded,
        theme.border_focus,
        format!(" Preview: {} ", theme.label),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = if model.advanced_ui {
        vec![
            anim::gradient_line(
                "◈ OBSCTL // THEME PREVIEW",
                theme.accent,
                theme.accent_alt,
                model.anim.frame,
                true,
            ),
            Line::raw(""),
            Line::from(vec![
                Span::styled("● LIVE", Style::default().fg(theme.danger)),
                Span::styled("   ◉ REC", Style::default().fg(theme.warning)),
                Span::styled("   ◆ SCENE ACTIVE", Style::default().fg(theme.accent)),
            ]),
            Line::from(Span::styled(
                "✓ connected",
                Style::default().fg(theme.success),
            )),
            Line::from(Span::styled(
                "⚠ warning",
                Style::default().fg(theme.warning),
            )),
            Line::from(Span::styled("ℹ info", Style::default().fg(theme.info))),
            Line::from(vec![
                Span::styled("▰▰▰▰", Style::default().fg(theme.success)),
                Span::styled("▰▰▰", Style::default().fg(theme.warning)),
                Span::styled("▰▰", Style::default().fg(theme.danger)),
                Span::styled("▱▱▱  -12.4 dB", Style::default().fg(theme.muted)),
            ]),
            Line::from(Span::styled(
                "CPU  ▁▂▃▅▄▆▇█▆▄   NET  ▁▁▂▃▅▇▅▃",
                Style::default().fg(theme.info),
            )),
            Line::from(Span::styled("muted text", Style::default().fg(theme.muted))),
            Line::raw(""),
            Line::from(Span::styled(
                " selected row ",
                Style::default()
                    .bg(theme.highlight_bg)
                    .fg(theme.highlight_fg),
            )),
        ]
    } else {
        vec![
            Line::styled("OBSCTL // THEME PREVIEW", Style::default().fg(theme.accent)),
            Line::raw(""),
            Line::from(vec![
                Span::styled("* LIVE", Style::default().fg(theme.danger)),
                Span::styled("   * REC", Style::default().fg(theme.warning)),
                Span::styled("   > SCENE ACTIVE", Style::default().fg(theme.accent)),
            ]),
            Line::styled("+ connected", Style::default().fg(theme.success)),
            Line::styled("! warning", Style::default().fg(theme.warning)),
            Line::styled("i info", Style::default().fg(theme.info)),
            Line::styled("#######...  -12.4 dB", Style::default().fg(theme.success)),
            Line::styled(
                "CPU  .-=+*#*+   NET  ..-=*#+-",
                Style::default().fg(theme.info),
            ),
            Line::styled("muted text", Style::default().fg(theme.muted)),
            Line::raw(""),
            Line::styled(
                " selected row ",
                Style::default()
                    .bg(theme.highlight_bg)
                    .fg(theme.highlight_fg),
            ),
        ]
    };

    let paragraph = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(paragraph, inner);
}
