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
    let focused = model.focus == FocusPanel::Scenes;

    let items: Vec<ListItem> = model
        .scenes()
        .iter()
        .map(|s| {
            let marker = if s.active { "▶ " } else { "  " };
            let mut spans = vec![
                Span::styled(
                    marker,
                    Style::default().fg(if s.active { theme.success } else { theme.muted }),
                ),
                Span::styled(s.name.as_str(), Style::default().fg(theme.fg)),
            ];
            if let Some(a) = &s.alias {
                spans.push(Span::styled(
                    format!(" ({a})"),
                    Style::default().fg(theme.muted),
                ));
            }
            if let Some(sc) = &s.shortcut {
                spans.push(Span::styled(
                    format!(" [{sc}]"),
                    Style::default().fg(theme.warning),
                ));
            }

            let style = if s.active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let border_style = if focused {
        Style::default().fg(theme.border_focus)
    } else {
        Style::default().fg(theme.border)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Scenes [s] · Enter to switch ")
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
    if !model.scenes().is_empty() {
        state.select(Some(model.scene_cursor));
    }

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style),
        area,
        &mut state,
    );
}
