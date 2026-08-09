use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span, Text},
    widgets::{List, ListItem, ListState},
};

use crate::tui::{
    model::{FocusPanel, TuiModel},
    theme::Theme,
    widgets::chrome,
};

const METER_WIDTH: usize = 20;
const FLOOR_DB: f32 = -60.0;

fn linear_to_db(level: f32) -> f32 {
    if level < 1e-7 {
        FLOOR_DB
    } else {
        (20.0 * level.log10()).max(FLOOR_DB)
    }
}

fn level_bar(level: f32, muted: bool, theme: Theme, advanced_ui: bool) -> Line<'static> {
    let db = linear_to_db(level);
    // Map [FLOOR_DB, 0 dBFS] → [0, METER_WIDTH] using dB scale so normal
    // speech (~-20 dBFS) fills roughly two-thirds of the bar.
    let fill_frac = ((db - FLOOR_DB) / (-FLOOR_DB)).clamp(0.0, 1.0);
    let filled = (fill_frac * METER_WIDTH as f32) as usize;
    let mut spans = vec![Span::styled(
        if advanced_ui { "  ╰─ " } else { "  +- " },
        Style::default().fg(theme.border),
    )];
    for index in 0..METER_WIDTH {
        if index < filled {
            let position = index as f32 / METER_WIDTH as f32;
            let color = if muted {
                theme.muted
            } else if position > 0.85 {
                theme.danger
            } else if position > 0.65 {
                theme.warning
            } else {
                crate::tui::anim::blend(theme.success, theme.info, position / 0.65)
            };
            spans.push(Span::styled(
                if advanced_ui { "▰" } else { "#" },
                Style::default().fg(color),
            ));
        } else {
            spans.push(Span::styled(
                if advanced_ui { "▱" } else { "." },
                Style::default().fg(theme.border),
            ));
        }
    }
    spans.push(Span::styled(
        format!("  {db:>5.1} dB"),
        Style::default().fg(if muted { theme.muted } else { theme.info }),
    ));
    Line::from(spans)
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Audio;

    let items: Vec<ListItem> = model
        .audio_inputs()
        .iter()
        .map(|a| {
            let mute_icon = match a.muted {
                Some(true) => Span::styled(
                    format!("{} ", model.symbol("🔇", "M")),
                    Style::default().fg(theme.danger),
                ),
                Some(false) => Span::styled(
                    format!("{} ", model.symbol("🔊", "A")),
                    Style::default().fg(theme.success),
                ),
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
                let bar = level_bar(level, muted, theme, model.advanced_ui);
                ListItem::new(Text::from(vec![info_line, bar]))
            } else {
                ListItem::new(info_line)
            }
        })
        .collect();

    let block = chrome::panel(
        model.symbol("🎚", "A"),
        "Audio Matrix",
        model.symbol("[a]  m mute  ←/→ gain", "[a]  m mute  </> gain"),
        model.audio_inputs().len(),
        focused,
        model,
    );

    let highlight_style = theme.selection_style(focused);

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
