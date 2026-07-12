use std::collections::BTreeMap;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use crate::tui::{model::TuiModel, theme::Theme};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
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
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for s in scenes {
            items.push(scene_row(s, theme));
        }
    }

    if !ungrouped.is_empty() {
        if !groups.is_empty() {
            items.push(ListItem::new(Line::styled(
                "[ungrouped]",
                Style::default().fg(theme.muted),
            )));
        }
        for s in &ungrouped {
            items.push(scene_row(s, theme));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Scene Map ");
    f.render_widget(List::new(items).block(block), area);
}

fn scene_row(s: &crate::obs::state::SceneState, theme: Theme) -> ListItem<'static> {
    let marker = if s.active { "▶ " } else { "  " };
    ListItem::new(Line::from(vec![
        Span::styled(
            marker,
            Style::default().fg(if s.active { theme.success } else { theme.muted }),
        ),
        Span::styled(s.name.clone(), Style::default().fg(theme.fg)),
    ]))
}
