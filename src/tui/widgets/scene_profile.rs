//! The scene-profile editor: a modal that floats over the dashboard while the
//! user picks which scenes a named profile hides.
//!
//! It is a popup rather than a view of its own — like the which-key overlay
//! and unlike the settings screen — so the dashboard underneath keeps
//! rendering and the editor's own two cursors stay out of the panel cursors.
//! What it draws comes from [`TuiModel::scene_profile_rows`], the same rows
//! the model's tests read back, so the picture and the assertions cannot come
//! apart.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use rust_i18n::t;

use crate::tui::{
    model::{
        SceneProfileEditor, SceneProfileRow, SceneProfileRowKind, SceneProfileStage, TuiModel,
    },
    widgets::chrome,
};

/// Share of the frame the modal covers, and the size below which it stops
/// shrinking. A profile name plus its hidden count needs about forty columns
/// before it starts wrapping, and twelve rows leaves room for a border, the
/// footer hint and a handful of scenes.
const WIDTH_PERCENT: u16 = 70;
const HEIGHT_PERCENT: u16 = 70;
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 12;

/// Draw the editor over `area`, or nothing at all when it is closed.
///
/// Called last in the frame, after the which-key popup, so nothing the
/// dashboard draws can land on top of it.
///
/// Returns where the row list ended up and the scroll offset Ratatui settled
/// on, which is what [`crate::tui::mouse`] maps a click row through. The rect
/// is the list's content — the modal draws it as a section of its interior,
/// not as a bordered panel — and is zero-sized when the editor is closed.
#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) -> (Rect, usize) {
    let Some(editor) = model.scene_profile.as_ref() else {
        return (Rect::default(), 0);
    };

    let theme = model.theme;
    let popup = centred(area);
    let block = chrome::ascii_aware(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border_focus))
            .style(Style::default().bg(theme.bg))
            .title(chrome::heading(
                model,
                title(model, editor.stage, editor.editing.as_deref()),
                theme.accent,
                theme.accent_alt,
            )),
        model,
    );

    // The modal covers live panels, so clear what is underneath before
    // drawing: without this the dashboard bleeds through wherever the popup
    // does not paint a cell of its own.
    f.render_widget(Clear, popup);
    let Some(inner) = chrome::frame(f, popup, block) else {
        return (Rect::default(), 0);
    };

    // The name is shown on the toggle stage too, not only while it is being
    // typed: it is what Enter is about to save under, and on a brand-new
    // profile the title says "New Scene Profile" rather than the name.
    let naming_rows = match editor.stage {
        SceneProfileStage::Picker => 0,
        SceneProfileStage::Scenes | SceneProfileStage::Naming => 1,
    };
    // The footer hint spells out every key of the stage, so on a narrow
    // terminal it needs more than one row — cut off, it would stop naming the
    // key the user is looking for, which is the only thing it is there for.
    let hint = hint_line(model, editor);
    let hint_rows = wrapped_rows(hint.width(), inner.width);
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(naming_rows),
            Constraint::Min(1),
            Constraint::Length(hint_rows),
        ])
        .split(inner);

    if naming_rows > 0 {
        f.render_widget(name_line(model, editor), sections[0]);
    }
    let offset = render_rows(f, sections[1], model);
    f.render_widget(Paragraph::new(hint).wrap(Wrap { trim: true }), sections[2]);
    (sections[1], offset)
}

/// Rows `width` columns of text take when wrapped into `available` columns.
///
/// Capped at three: past that the hint would be eating the list it belongs
/// to, and a terminal that narrow has bigger problems.
fn wrapped_rows(width: usize, available: u16) -> u16 {
    if available == 0 {
        return 1;
    }
    u16::try_from(width.div_ceil(usize::from(available)))
        .unwrap_or(u16::MAX)
        .clamp(1, 3)
}

/// `percent` per cent of `total`, computed in `u32`.
///
/// A `Rect` dimension is a `u16`, and `937 * 70` already passes 65535 — so
/// doing this multiplication in `u16` panics on a terminal 937 columns or
/// rows wide in a debug build, and silently wraps to a nonsense size in a
/// release one. Widening first is the whole fix; the result always fits back
/// into a `u16` because it is a fraction of one.
fn share(total: u16, percent: u16) -> u16 {
    u16::try_from(u32::from(total) * u32::from(percent) / 100).unwrap_or(total)
}

/// The popup's rect: a fixed share of the frame, never below the minimum the
/// content needs, and never larger than the frame it sits in.
fn centred(area: Rect) -> Rect {
    let width = share(area.width, WIDTH_PERCENT)
        .max(MIN_WIDTH)
        .min(area.width);
    let height = share(area.height, HEIGHT_PERCENT)
        .max(MIN_HEIGHT)
        .min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// What the border says the user is doing: choosing a profile, or editing a
/// particular one — by name when it already exists, as "new" when it does not.
fn title(model: &TuiModel, stage: SceneProfileStage, editing: Option<&str>) -> String {
    let text = match (stage, editing) {
        (SceneProfileStage::Picker, _) => t!("tui.panels.scene_profiles.title").into_owned(),
        (_, Some(name)) => t!("tui.panels.scene_profiles.title_editing", name = name).into_owned(),
        (_, None) => t!("tui.panels.scene_profiles.title_new").into_owned(),
    };
    format!(" {} ", chrome::typographic(model, &text))
}

/// The name field. Only the naming stage carries the block cursor, so the
/// toggle stage shows the same text without inviting a keystroke that would
/// go somewhere else.
fn name_line(model: &TuiModel, editor: &SceneProfileEditor) -> Line<'static> {
    let theme = model.theme;
    let name = editor.name.value.clone();
    let mut spans = vec![
        Span::styled(
            format!(" {}", t!("tui.panels.scene_profiles.name_prompt")),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            name,
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    if editor.stage == SceneProfileStage::Naming {
        spans.push(Span::styled(
            model.symbol("█", "_"),
            Style::default().fg(theme.accent),
        ));
    }
    Line::from(spans)
}

/// The per-stage footer telling the user which keys do what here.
///
/// A delete waiting to be confirmed replaces it. That footer is the only place
/// the modal can put the question, and it has to name the profile: the picker
/// cursor sits on a highlighted row, but "the highlighted row" is not what a
/// user wants to be trusting when the answer destroys a profile with no undo.
fn hint_line(model: &TuiModel, editor: &SceneProfileEditor) -> Line<'static> {
    if let Some(name) = editor.pending_delete.as_deref() {
        let key = model.symbol(
            "tui.panels.scene_profiles.hint_confirm_delete",
            "tui.panels.scene_profiles.hint_confirm_delete_ascii",
        );
        return Line::from(Span::styled(
            chrome::typographic(model, &t!(key, name = name)),
            Style::default()
                .fg(model.theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let key = match editor.stage {
        SceneProfileStage::Picker => model.symbol(
            "tui.panels.scene_profiles.hint_picker",
            "tui.panels.scene_profiles.hint_picker_ascii",
        ),
        SceneProfileStage::Scenes => model.symbol(
            "tui.panels.scene_profiles.hint_scenes",
            "tui.panels.scene_profiles.hint_scenes_ascii",
        ),
        SceneProfileStage::Naming => model.symbol(
            "tui.panels.scene_profiles.hint_naming",
            "tui.panels.scene_profiles.hint_naming_ascii",
        ),
    };
    chrome::placeholder_line(model, chrome::typographic(model, &t!(key)))
}

/// The list itself, with the cursor of whichever stage is up.
///
/// Selection is taken from the rows' own `selected` flag rather than from the
/// editor's cursors, so the highlight lands on the row the model says is
/// current even if a stage ever grew a row the cursor does not count.
///
/// Returns the scroll offset Ratatui settled on, for the mouse code.
fn render_rows(f: &mut Frame, area: Rect, model: &TuiModel) -> usize {
    let rows = model.scene_profile_rows();
    let selected = rows.iter().position(|row| row.selected);
    let mut items: Vec<ListItem> = rows.iter().map(|row| row_item(model, row)).collect();

    // The picker always has its "new scene profile" row, so an empty list of
    // profiles is a note under that row rather than an empty pane. It is
    // appended after the rows the cursor is clamped against, which is why it
    // can never be selected.
    let on_picker = model
        .scene_profile
        .as_ref()
        .is_some_and(|editor| editor.stage == SceneProfileStage::Picker);
    if on_picker && model.scene_profiles().is_empty() {
        items.push(ListItem::new(chrome::placeholder_line(
            model,
            chrome::typographic(model, &t!("tui.panels.scene_profiles.empty")),
        )));
    }

    let mut state = ListState::default();
    state.select(selected);
    f.render_stateful_widget(
        List::new(items).highlight_style(model.theme.selection_style(true)),
        area,
        &mut state,
    );
    state.offset()
}

fn row_item(model: &TuiModel, row: &SceneProfileRow) -> ListItem<'static> {
    let theme = model.theme;
    match row.kind {
        SceneProfileRowKind::NewProfile => ListItem::new(Line::from(Span::styled(
            format!(
                " {}",
                chrome::typographic(
                    model,
                    &t!(model.symbol(
                        "tui.panels.scene_profiles.new_entry",
                        "tui.panels.scene_profiles.new_entry_ascii"
                    ))
                )
            ),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))),
        SceneProfileRowKind::Profile {
            active,
            hidden_count,
            listed_count,
        } => {
            // The count has to be the one the user can check against the
            // dashboard, so a profile still naming a scene OBS has renamed
            // away says how many of its entries land rather than promising
            // more than will disappear.
            let count = if listed_count > hidden_count {
                t!(
                    "tui.panels.scene_profiles.hidden_count_partial",
                    count = hidden_count.to_string(),
                    listed = listed_count.to_string()
                )
            } else {
                t!(
                    "tui.panels.scene_profiles.hidden_count",
                    count = hidden_count.to_string()
                )
            };
            let mut spans = vec![
                Span::styled(format!("  {}", row.label), Style::default().fg(theme.fg)),
                Span::styled(format!("  {count}"), Style::default().fg(theme.muted)),
            ];
            if active {
                spans.push(Span::styled(
                    format!(
                        "  {}",
                        t!(model.symbol(
                            "tui.panels.scene_profiles.active_marker",
                            "tui.panels.scene_profiles.active_marker_ascii"
                        ))
                    ),
                    Style::default().fg(theme.success),
                ));
            }
            let style = if active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        }
        SceneProfileRowKind::Scene { hidden, current } => {
            // A hidden scene is drawn the way it will look once the profile is
            // active: still listed, but pushed into the background.
            let (marker, name_style, state_key) = if hidden {
                (
                    model.symbol("○", "-"),
                    Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                    "tui.panels.scene_profiles.state_hidden",
                )
            } else {
                (
                    model.symbol("●", "*"),
                    Style::default().fg(theme.fg),
                    "tui.panels.scene_profiles.state_visible",
                )
            };
            let spans = vec![
                Span::styled(
                    format!(" {marker} "),
                    Style::default().fg(if hidden { theme.muted } else { theme.success }),
                ),
                Span::styled(row.label.clone(), name_style),
                Span::styled(
                    format!("  {}", t!(state_key)),
                    Style::default().fg(theme.muted),
                ),
            ];
            // The scene OBS is showing right now stays bold whichever side of
            // the toggle it is on, so the user can see what they are hiding.
            let style = if current {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(spans)).style(style)
        }
        // An entry naming no scene OBS has is drawn as a warning rather than
        // as another scene: it is not a choice between hiding and showing, it
        // is config left over from a rename, and `t` is how it goes away.
        SceneProfileRowKind::MissingScene => {
            let spans = vec![
                Span::styled(
                    format!(" {} ", model.symbol("⚠", "!")),
                    Style::default().fg(theme.warning),
                ),
                Span::styled(
                    row.label.clone(),
                    Style::default().fg(theme.muted).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!(
                        "  {}",
                        chrome::typographic(
                            model,
                            &t!(model.symbol(
                                "tui.panels.scene_profiles.state_missing",
                                "tui.panels.scene_profiles.state_missing_ascii"
                            ))
                        )
                    ),
                    Style::default().fg(theme.warning),
                ),
            ];
            ListItem::new(Line::from(spans))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Rect` dimension is a `u16` and the popup is a percentage of it, so
    /// the multiplication has to be widened first: `937 * 70` is already past
    /// what a `u16` holds, which used to panic the TUI the instant the modal
    /// opened on a terminal that wide.
    #[test]
    fn the_popup_is_sized_without_overflowing_on_a_very_wide_terminal() {
        let popup = centred(Rect {
            x: 0,
            y: 0,
            width: 1000,
            height: 1000,
        });

        assert_eq!(popup.width, 700);
        assert_eq!(popup.height, 700);
        assert_eq!(popup.x, 150, "and it is still centred");
        assert_eq!(popup.y, 150);
    }

    /// The floor and the ceiling still hold: a small terminal gets the
    /// minimum the content needs, clipped to what there is.
    #[test]
    fn a_small_terminal_gets_the_minimum_size_clipped_to_the_frame() {
        let popup = centred(Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 8,
        });

        assert_eq!(popup.width, 30);
        assert_eq!(popup.height, 8);
    }
}
