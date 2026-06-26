use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::tui::model::{FocusPanel, TuiModel};

const METER_WIDTH: usize = 20;
// dBFS thresholds matching OBS's built-in meter
const FLOOR_DB: f32 = -60.0;
const YELLOW_DB: f32 = -20.0;
const RED_DB: f32 = -9.0;

fn linear_to_db(level: f32) -> f32 {
    if level < 1e-7 {
        FLOOR_DB
    } else {
        (20.0 * level.log10()).max(FLOOR_DB)
    }
}

fn level_bar(level: f32, muted: bool) -> Line<'static> {
    let db = linear_to_db(level);
    // Map [FLOOR_DB, 0 dBFS] → [0, METER_WIDTH] using dB scale so normal
    // speech (~-20 dBFS) fills roughly two-thirds of the bar.
    let fill_frac = ((db - FLOOR_DB) / (-FLOOR_DB)).clamp(0.0, 1.0);
    let filled = (fill_frac * METER_WIDTH as f32) as usize;
    let empty = METER_WIDTH - filled;
    let bar_color = if muted {
        Color::DarkGray
    } else if db > RED_DB {
        Color::Red
    } else if db > YELLOW_DB {
        Color::Yellow
    } else {
        Color::Green
    };
    let filled_str: String = std::iter::repeat('█').take(filled).collect();
    let empty_str: String = std::iter::repeat('░').take(empty).collect();
    Line::from(vec![
        Span::raw("  "),
        Span::styled(filled_str, Style::default().fg(bar_color)),
        Span::styled(empty_str, Style::default().fg(Color::DarkGray)),
    ])
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let focused = model.focus == FocusPanel::Audio;

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

            let info_line = Line::from(spans);

            // Show a level bar if we have meter data for this input.
            if let Some(&level) = model.meter_levels.get(&a.name) {
                let muted = a.muted.unwrap_or(false);
                let bar = level_bar(level, muted);
                ListItem::new(Text::from(vec![info_line, bar]))
            } else {
                ListItem::new(info_line)
            }
        })
        .collect();

    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Audio [a] · m mute · ←/→ volume ")
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
    if !model.audio_inputs().is_empty() {
        state.select(Some(model.audio_cursor));
    }

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style),
        area,
        &mut state,
    );
}
