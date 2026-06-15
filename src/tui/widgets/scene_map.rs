use std::collections::BTreeMap;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let mut groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
    let mut ungrouped = Vec::new();

    for s in model.scenes() {
        if let Some(g) = &s.group {
            groups.entry(g.clone()).or_default().push(s);
        } else {
            ungrouped.push(s);
        }
    }

    let mut items: Vec<ListItem> = Vec::new();

    for (group, scenes) in &groups {
        items.push(ListItem::new(Line::styled(
            format!("[{group}]"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for s in scenes {
            let marker = if s.active { "▶ " } else { "  " };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(if s.active {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(s.name.clone()),
            ])));
        }
    }

    if !ungrouped.is_empty() {
        if !groups.is_empty() {
            items.push(ListItem::new(Line::styled(
                "[ungrouped]",
                Style::default().fg(Color::DarkGray),
            )));
        }
        for s in &ungrouped {
            let marker = if s.active { "▶ " } else { "  " };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(if s.active {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                ),
                Span::raw(s.name.clone()),
            ])));
        }
    }

    let block = Block::default().borders(Borders::ALL).title(" Scene Map ");
    f.render_widget(List::new(items).block(block), area);
}
