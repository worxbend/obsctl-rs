use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::tui::{
    anim,
    model::{FocusPanel, TuiModel},
};

/// How many ticks the just-switched-to scene stays highlighted.
const FLASH_DURATION_TICKS: u64 = 8;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Scenes;

    let items: Vec<ListItem> = model
        .scenes()
        .iter()
        .map(|s| {
            let marker = if s.active { "▶ " } else { "  " };
            let flash_t = flash_intensity(model, &s.name);
            let active_color = if flash_t > 0.0 {
                anim::blend(theme.success, theme.accent, flash_t)
            } else {
                theme.success
            };
            let mut spans = vec![
                Span::styled(
                    marker,
                    Style::default().fg(if s.active { active_color } else { theme.muted }),
                ),
                Span::styled(
                    s.name.as_str(),
                    Style::default().fg(if flash_t > 0.0 {
                        active_color
                    } else {
                        theme.fg
                    }),
                ),
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

/// 1.0 right after `name` becomes active, decaying linearly to 0.0 over
/// `FLASH_DURATION_TICKS`; 0.0 if `name` isn't the scene currently flashing.
fn flash_intensity(model: &TuiModel, name: &str) -> f32 {
    let Some((flashing_name, started_at)) = &model.scene_flash else {
        return 0.0;
    };
    if flashing_name != name {
        return 0.0;
    }
    let elapsed = model.anim.frame.saturating_sub(*started_at);
    if elapsed >= FLASH_DURATION_TICKS {
        return 0.0;
    }
    1.0 - (elapsed as f32 / FLASH_DURATION_TICKS as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_flash(started_at: u64, now: u64) -> TuiModel {
        let mut model = TuiModel::default();
        model.scene_flash = Some(("Cam".to_string(), started_at));
        model.anim.frame = now;
        model
    }

    #[test]
    fn flash_intensity_is_zero_for_unrelated_scene() {
        let model = model_with_flash(0, 0);
        assert_eq!(flash_intensity(&model, "Main"), 0.0);
    }

    #[test]
    fn flash_intensity_is_full_at_start() {
        let model = model_with_flash(5, 5);
        assert_eq!(flash_intensity(&model, "Cam"), 1.0);
    }

    #[test]
    fn flash_intensity_decays_to_zero_after_duration() {
        let model = model_with_flash(0, FLASH_DURATION_TICKS);
        assert_eq!(flash_intensity(&model, "Cam"), 0.0);
    }

    #[test]
    fn flash_intensity_is_none_when_no_flash_active() {
        let model = TuiModel::default();
        assert_eq!(flash_intensity(&model, "Cam"), 0.0);
    }
}
