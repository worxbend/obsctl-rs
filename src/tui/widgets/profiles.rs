use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::tui::model::{FocusPanel, TuiModel};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Profiles;
    let current = model.current_profile();

    let items: Vec<ListItem> = model
        .profiles()
        .iter()
        .map(|name| {
            let active = Some(name.as_str()) == current;
            let marker = if active { "▶ " } else { "  " };
            let style = if active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(if active { theme.success } else { theme.muted }),
                ),
                Span::styled(name.as_str(), Style::default().fg(theme.fg)),
            ]))
            .style(style)
        })
        .collect();

    let border_style = if focused {
        Style::default().fg(theme.border_focus)
    } else {
        Style::default().fg(theme.border)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Profiles [p] · Enter to switch ")
        .border_style(border_style);

    let highlight_style = if focused {
        Style::default()
            .bg(theme.highlight_bg)
            .fg(theme.highlight_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::DIM)
    };

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
