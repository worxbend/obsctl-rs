use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
};

use rust_i18n::t;

use crate::tui::{
    anim,
    model::{FocusPanel, TuiModel},
    widgets::chrome,
};

/// How many ticks the just-switched-to scene stays highlighted.
const FLASH_DURATION_TICKS: u64 = 8;

/// Returns the scroll offset Ratatui settled on, for the mouse code to map a
/// click to a row. See [`crate::tui::mouse`].
#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) -> usize {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Scenes;

    let items: Vec<ListItem> = model
        .scenes()
        .iter()
        .enumerate()
        .map(|(index, s)| {
            let marker = if s.active {
                model.symbol("▶", ">")
            } else {
                model.symbol("◇", " ")
            };
            let flash_t = flash_intensity(model, &s.name);
            let active_color = if flash_t > 0.0 {
                anim::blend(theme.success, theme.accent, flash_t)
            } else {
                theme.success
            };
            let mut spans = vec![
                Span::styled(
                    format!(" {:02} ", index + 1),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(
                    format!("{marker} "),
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
            if let Some(group) = &s.group {
                spans.push(Span::styled(
                    format!(
                        "  {}{group}{}",
                        chrome::glyph(model, "⟨", "["),
                        chrome::glyph(model, "⟩", "]")
                    ),
                    Style::default().fg(theme.accent_alt),
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

    let title = t!("tui.panels.scenes.title");
    let hint = t!(model.symbol("tui.panels.scenes.hint", "tui.panels.scenes.hint_ascii"));
    let block = chrome::panel(
        model.symbol("🎬", "S"),
        &title,
        &hint,
        model.scenes().len(),
        focused,
        model,
    );

    let highlight_style = theme.selection_style(focused);

    let mut state = ListState::default();
    if !model.scenes().is_empty() {
        state.select(Some(model.panel_cursor(FocusPanel::Scenes)));
    }

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style),
        area,
        &mut state,
    );
    state.offset()
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
