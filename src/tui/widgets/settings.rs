use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use crate::tui::{model::TuiModel, theme};

/// Full-screen settings view — currently just the theme picker, styled
/// after btop's theme switcher: arrow keys live-preview a theme across the
/// whole UI, Enter confirms and persists it, Esc reverts to whatever was
/// active before opening this view.
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focus))
        .title(" Settings — ↑/↓ preview · Enter apply · Esc/F2 cancel ");
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    render_theme_list(f, sections[0], model);
    render_preview(f, sections[1], model);
}

fn render_theme_list(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let items: Vec<ListItem> = theme::ALL
        .iter()
        .map(|t| {
            let swatch = Line::from(vec![
                Span::styled("██", Style::default().fg(t.accent)),
                Span::styled("██", Style::default().fg(t.success)),
                Span::styled("██", Style::default().fg(t.warning)),
                Span::styled("██", Style::default().fg(t.danger)),
                Span::raw("  "),
                Span::raw(t.label),
            ]);
            ListItem::new(swatch)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Themes ");

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
}

fn render_preview(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focus))
        .title(format!(" Preview: {} ", theme.label));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = vec![
        Line::from(Span::styled(
            "obsctl-rs",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled("● LIVE", Style::default().fg(theme.danger))),
        Line::from(Span::styled(
            "✓ connected",
            Style::default().fg(theme.success),
        )),
        Line::from(Span::styled(
            "⚠ warning",
            Style::default().fg(theme.warning),
        )),
        Line::from(Span::styled("ℹ info", Style::default().fg(theme.info))),
        Line::from(Span::styled("muted text", Style::default().fg(theme.muted))),
        Line::raw(""),
        Line::from(Span::styled(
            " selected row ",
            Style::default()
                .bg(theme.highlight_bg)
                .fg(theme.highlight_fg),
        )),
    ];

    let paragraph = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(paragraph, inner);
}
