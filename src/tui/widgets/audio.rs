use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::tui::{
    model::{FocusPanel, TuiModel},
    theme::Theme,
};

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

fn level_bar(level: f32, muted: bool, theme: Theme) -> Line<'static> {
    let db = linear_to_db(level);
    // Map [FLOOR_DB, 0 dBFS] → [0, METER_WIDTH] using dB scale so normal
    // speech (~-20 dBFS) fills roughly two-thirds of the bar.
    let fill_frac = ((db - FLOOR_DB) / (-FLOOR_DB)).clamp(0.0, 1.0);
    let filled = (fill_frac * METER_WIDTH as f32) as usize;
    let empty = METER_WIDTH - filled;
    let bar_color = if muted {
        theme.muted
    } else if db > RED_DB {
        theme.danger
    } else if db > YELLOW_DB {
        theme.warning
    } else {
        theme.success
    };
    let filled_str = "█".repeat(filled);
    let empty_str = "░".repeat(empty);
    Line::from(vec![
        Span::raw("  "),
        Span::styled(filled_str, Style::default().fg(bar_color)),
        Span::styled(empty_str, Style::default().fg(theme.muted)),
    ])
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Audio;

    let items: Vec<ListItem> = model
        .audio_inputs()
        .iter()
        .map(|a| {
            let mute_icon = match a.muted {
                Some(true) => Span::styled("🔇 ", Style::default().fg(theme.danger)),
                Some(false) => Span::styled("🔊 ", Style::default().fg(theme.success)),
                None => Span::raw("   "),
            };

            let vol = match a.volume_percent {
                Some(v) => format!(" {v}%"),
                None => String::new(),
            };

            let mut spans = vec![
                mute_icon,
                Span::styled(a.name.as_str(), Style::default().fg(theme.fg)),
            ];

            if let Some(al) = &a.alias {
                spans.push(Span::styled(
                    format!(" ({al})"),
                    Style::default().fg(theme.muted),
                ));
            }
            if let Some(sc) = &a.shortcut {
                spans.push(Span::styled(
                    format!(" [{sc}]"),
                    Style::default().fg(theme.warning),
                ));
            }
            if !vol.is_empty() {
                spans.push(Span::styled(vol, Style::default().fg(theme.info)));
            }

            let info_line = Line::from(spans);

            // Show a level bar if we have meter data for this input.
            if let Some(&level) = model.meter_levels.get(&a.name) {
                let muted = a.muted.unwrap_or(false);
                let bar = level_bar(level, muted, theme);
                ListItem::new(Text::from(vec![info_line, bar]))
            } else {
                ListItem::new(info_line)
            }
        })
        .collect();

    let border_style = if focused {
        Style::default().fg(theme.border_focus)
    } else {
        Style::default().fg(theme.border)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Audio [a] · m mute · ←/→ volume ")
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
