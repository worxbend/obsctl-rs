use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use crate::tui::model::TuiModel;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let items: Vec<ListItem> = model
        .audio_inputs()
        .iter()
        .map(|a| {
            let mute_icon = match a.muted {
                Some(true) => Span::styled("🔇 ", Style::default().fg(Color::Red)),
                Some(false) => Span::styled("🔊 ", Style::default().fg(Color::Green)),
                None => Span::raw("   "),
            };

            let vol = match a.volume_percent {
                Some(v) => format!(" {v}%"),
                None => String::new(),
            };

            let mut spans = vec![mute_icon, Span::raw(a.name.clone())];

            if let Some(al) = &a.alias {
                spans.push(Span::styled(
                    format!(" ({al})"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if let Some(sc) = &a.shortcut {
                spans.push(Span::styled(
                    format!(" [{sc}]"),
                    Style::default().fg(Color::Yellow),
                ));
            }
            if !vol.is_empty() {
                spans.push(Span::styled(vol, Style::default().fg(Color::Blue)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = Block::default().borders(Borders::ALL).title(" Audio ");
    f.render_widget(List::new(items).block(block), area);
}
