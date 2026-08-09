use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
};

use crate::tui::{
    model::{FocusPanel, TuiModel},
    widgets::chrome,
};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Profiles;
    let current = model.current_profile();

    let items: Vec<ListItem> = model
        .profiles()
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let active = Some(name.as_str()) == current;
            let marker = if active {
                model.symbol("▶", ">")
            } else {
                model.symbol("◇", " ")
            };
            let style = if active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {:02} ", index + 1),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(if active { theme.success } else { theme.muted }),
                ),
                Span::styled(name.as_str(), Style::default().fg(theme.fg)),
            ]))
            .style(style)
        })
        .collect();

    let block = chrome::panel(
        model.symbol("🎛", "P"),
        "Profiles",
        model.symbol("[p]  ↵ switch", "[p]  Enter switch"),
        model.profiles().len(),
        focused,
        model,
    );

    let highlight_style = theme.selection_style(focused);

    let mut state = ListState::default();
    if !model.profiles().is_empty() {
        state.select(Some(model.profile_cursor));
    }

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style),
        area,
        &mut state,
    );
}
