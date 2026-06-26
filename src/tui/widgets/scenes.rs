use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::tui::model::{FocusPanel, TuiModel};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let focused = model.focus == FocusPanel::Scenes;

    let items: Vec<ListItem> = model
        .scenes()
        .iter()
        .map(|s| {
            let marker = if s.active { "▶ " } else { "  " };
            let mut spans = vec![
                Span::styled(
                    marker,
                    Style::default().fg(if s.active {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(s.name.clone()),
            ];
            if let Some(a) = &s.alias {
                spans.push(Span::styled(
                    format!(" ({a})"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if let Some(sc) = &s.shortcut {
                spans.push(Span::styled(
                    format!(" [{sc}]"),
                    Style::default().fg(Color::Yellow),
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
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Scenes [s] · Enter to switch ")
        .border_style(border_style);

    let highlight_style = if focused {
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
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
