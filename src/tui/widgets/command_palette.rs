use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

use crate::tui::model::TuiModel;

const MAX_VISIBLE_COMPLETIONS: usize = 8;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Command Palette ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(result) = &model.last_result {
        let style = if result.starts_with("error") {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::styled(result.clone(), style));
    } else {
        lines.push(Line::raw(""));
    }

    let prompt_line = if model.command_palette.active {
        Line::from(vec![
            Span::styled(
                "> ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(model.command_palette.input.clone()),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ])
    } else {
        Line::from(vec![Span::styled(
            " / or : to open command palette  q to quit  r reload  D dump",
            Style::default().fg(Color::DarkGray),
        )])
    };
    lines.push(prompt_line);

    f.render_widget(Paragraph::new(lines), inner);

    if model.command_palette.active && !model.command_palette.completions.is_empty() {
        render_completions_popup(f, area, model);
    }
}

fn render_completions_popup(f: &mut Frame, palette_area: Rect, model: &TuiModel) {
    let completions = &model.command_palette.completions;
    let visible_count = completions.len().min(MAX_VISIBLE_COMPLETIONS);

    let max_item_len = completions.iter().map(|c| c.len()).max().unwrap_or(10);
    let popup_width = (max_item_len + 4).min(palette_area.width.saturating_sub(4) as usize) as u16;
    let popup_height = (visible_count + 2) as u16; // +2 for borders

    // Float above the palette; clamp to top of screen
    let popup_y = palette_area.y.saturating_sub(popup_height);
    let popup_x = palette_area.x + 2;

    let popup_rect = Rect {
        x: popup_x.min(palette_area.x + palette_area.width.saturating_sub(popup_width + 2)),
        y: popup_y,
        width: popup_width,
        height: popup_height,
    };

    let items: Vec<ListItem> = completions
        .iter()
        .take(MAX_VISIBLE_COMPLETIONS)
        .enumerate()
        .map(|(i, c)| {
            let style = if Some(i) == model.command_palette.completion_idx {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(format!(" {c}"), style)))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(" Tab ", Style::default().fg(Color::DarkGray))),
    );

    f.render_widget(Clear, popup_rect);
    f.render_widget(list, popup_rect);
}
