use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
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

    let block = Block::default().borders(Borders::ALL).title(" Scenes ");
    f.render_widget(List::new(items).block(block), area);
}
