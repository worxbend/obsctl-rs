use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, ListItem, Padding, Wrap},
};

use rust_i18n::t;

use crate::tui::{
    anim,
    model::{FocusPanel, TuiModel},
    widgets::{chrome, name_list},
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
            let flash_t = flash_intensity(model, &s.name);
            let active_color = if flash_t > 0.0 {
                anim::blend(theme.success, theme.accent, flash_t)
            } else {
                theme.success
            };
            let mut spans = Vec::from(name_list::row_prefix(model, index, s.active, active_color));
            spans.push(Span::styled(
                s.name.as_str(),
                Style::default().fg(if flash_t > 0.0 {
                    active_color
                } else {
                    theme.fg
                }),
            ));
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
    let hint = chrome::phrase(
        model,
        "tui.panels.scenes.hint",
        "tui.panels.scenes.hint_ascii",
    );
    let badge = filter_badge(model);
    let block = chrome::panel_badged(
        model.symbol("🎬", "S"),
        &title,
        badge.as_deref(),
        &hint,
        model.scenes().len(),
        focused,
        model,
    );

    if items.is_empty() && hidden_count(model) > 0 {
        return render_everything_hidden(f, area, model, block);
    }
    name_list::render_rows(f, area, model, FocusPanel::Scenes, block, items)
}

/// How many scenes the daemon knows about that this panel is not listing.
fn hidden_count(model: &TuiModel) -> usize {
    model
        .all_scenes()
        .len()
        .saturating_sub(model.scenes().len())
}

/// The title badge naming what is being filtered out, or `None` when the user
/// is looking at every scene there is.
///
/// Two things can shorten the list: the scene profile that is switched on, or
/// a scene's own `hidden:` flag in the config. The first is worth naming — it
/// is the one the user turned on and can turn off again — and the second at
/// least gets a count, so a list that is missing rows never looks like a list
/// that lost them.
///
/// An active profile is named whether or not it is hiding anything at the
/// moment. This badge is the only thing on the dashboard that says a profile
/// is in effect, and a profile hiding nothing — one whose entries all name
/// scenes OBS has since renamed, say — is exactly the case a user needs told:
/// without the badge it looks like no profile at all, and they go hunting for
/// why the one they switched on is not working.
fn filter_badge(model: &TuiModel) -> Option<String> {
    let count = hidden_count(model);
    let text = match (model.active_scene_profile(), count) {
        (Some(name), 0) => t!("tui.panels.scenes.badge_profile_empty", name = name),
        (Some(name), count) => t!(
            "tui.panels.scenes.badge_profile",
            name = name,
            count = count
        ),
        (None, 0) => return None,
        (None, count) => t!("tui.panels.scenes.badge_filtered", count = count),
    };
    Some(chrome::typographic(model, &text))
}

/// The body of a Scenes panel whose every scene is filtered out.
///
/// An empty list draws as blank space, which says nothing about why — and
/// "why" is the whole question when a profile has just hidden the lot. This
/// says which profile did it and which key undoes it. Returns a scroll offset
/// of zero because there are no rows for a click to land on.
fn render_everything_hidden(f: &mut Frame, area: Rect, model: &TuiModel, block: Block) -> usize {
    let Some(inner) = chrome::frame(f, area, block) else {
        return 0;
    };
    let text = match model.active_scene_profile() {
        Some(name) => t!("tui.panels.scenes.all_hidden_by_profile", name = name),
        None => t!("tui.panels.scenes.all_hidden"),
    };
    // Wrapped rather than clipped: this is the one place in the panel that has
    // a sentence to deliver, and the Scenes pane is only half the dashboard
    // wide. The padding is what keeps the second and third lines lined up
    // under the first instead of running flush against the border.
    f.render_widget(
        chrome::placeholder(model, chrome::typographic(model, &text))
            .wrap(Wrap { trim: false })
            .block(Block::default().padding(Padding::horizontal(1))),
        inner,
    );
    0
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
