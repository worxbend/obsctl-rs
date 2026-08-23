//! Mouse navigation: turning a click, wheel tick, or right-click into the
//! same [`TuiAction`]s the keyboard produces.
//!
//! Hit-testing needs to know where the last frame actually drew things, so
//! [`render`](crate::tui::render::render) hands back a [`Hitboxes`] snapshot that this
//! module resolves against. Row lookup mirrors Ratatui's own list scrolling
//! (see [`first_visible_index`]) — the widgets build a fresh `ListState` each
//! frame, so the visible window is a pure function of the cursor and the
//! pane height.

use ratatui::layout::{Position, Rect};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::tui::{
    input::TuiAction,
    model::{FocusPanel, SceneProfileStage, TuiModel},
};

/// Rows a wheel tick moves. Three matches the usual terminal wheel step.
const WHEEL_ROWS: usize = 3;

/// Which screen the last frame drew, since that decides what a click means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HitView {
    #[default]
    Main,
    Settings,
    /// The "daemon unavailable" screen — a click anywhere retries.
    Disconnected,
    /// The scene-profile editor is up. It is modal, so it answers every
    /// mouse event itself and nothing reaches the dashboard behind it.
    SceneProfile,
}

/// Where the last frame put each interactive region. Zero-sized rects are
/// treated as absent, which is what [`Rect::default`] gives for a screen
/// that never drew that panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hitboxes {
    pub view: HitView,
    pub scenes: Rect,
    pub audio: Rect,
    pub profiles: Rect,
    pub collections: Rect,
    pub logs: Rect,
    pub palette: Rect,
    pub settings_list: Rect,
    /// The scene-profile editor's list, as the *content* rect — the modal
    /// draws it inside its border rather than as a bordered panel of its own,
    /// so unlike the fields above this one has no border to subtract.
    pub scene_profile_list: Rect,
    /// Which item each scrollable list drew first, as reported by Ratatui
    /// after it laid the list out. See [`ListOffsets`].
    pub offsets: ListOffsets,
}

/// The scroll offset each list settled on in the frame these hitboxes
/// describe.
///
/// Taken from the `ListState` Ratatui writes back during rendering rather than
/// recomputed. The mouse code needs to know which item is drawn at the top of
/// a panel to turn a click row into an index, and it used to answer that with
/// a hand-port of Ratatui's `get_items_bounds`. Two consequences: the port had
/// to be kept in step with an unpinned `ratatui = "0.30"` by hand, and it was
/// derived from the selection rather than read from what was actually drawn,
/// so the two could disagree with nothing to notice. A minor version that
/// changed list scrolling would have moved every click in a scrolled panel to
/// the neighbouring row with the whole suite still green.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListOffsets {
    pub scenes: usize,
    pub profiles: usize,
    pub collections: usize,
    pub settings: usize,
    pub scene_profile: usize,
}

/// Which offset belongs to which panel, in one place.
///
/// The mapping used to be written out wherever it was needed, so a panel
/// added to the dashboard had to be remembered in several matches at once.
impl std::ops::Index<FocusPanel> for ListOffsets {
    type Output = usize;

    fn index(&self, panel: FocusPanel) -> &usize {
        match panel {
            FocusPanel::Scenes => &self.scenes,
            FocusPanel::Profiles => &self.profiles,
            FocusPanel::Collections => &self.collections,
            // The audio matrix is columns of vertical strips, not a list; it
            // maps clicks through `audio::strip_index_at` instead, so it has
            // no scroll offset of its own to report.
            FocusPanel::Audio => &0,
        }
    }
}

/// Which rect belongs to which panel — the same idea as the offsets above,
/// and the reason neither `panel_at` nor the half-page motions have to spell
/// the mapping out again.
impl std::ops::Index<FocusPanel> for Hitboxes {
    type Output = Rect;

    fn index(&self, panel: FocusPanel) -> &Rect {
        match panel {
            FocusPanel::Scenes => &self.scenes,
            FocusPanel::Audio => &self.audio,
            FocusPanel::Profiles => &self.profiles,
            FocusPanel::Collections => &self.collections,
        }
    }
}

impl Hitboxes {
    /// The panel `pos` landed in, with the area it was drawn in, or `None`
    /// for a click outside all four. Panels never overlap, so the first hit
    /// is the only hit.
    fn panel_at(&self, pos: Position) -> Option<(FocusPanel, Rect)> {
        FocusPanel::ALL
            .into_iter()
            .map(|panel| (panel, self[panel]))
            .find(|(_, area)| contains(*area, pos))
    }
}

fn contains(area: Rect, pos: Position) -> bool {
    area.width > 0 && area.height > 0 && area.contains(pos)
}

/// Content area of a bordered panel — every list here is drawn inside a
/// one-cell block border.
fn inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

/// Index of the item a click lands on. Every panel but the audio matrix is a
/// list of one-row items, so the lookup is a row lookup; the audio matrix
/// draws vertical channel strips side by side, so there it is a column
/// lookup against the same layout the widget used to draw them.
fn index_at(
    model: &TuiModel,
    hits: &Hitboxes,
    panel: FocusPanel,
    area: Rect,
    pos: Position,
) -> Option<usize> {
    match panel {
        FocusPanel::Audio => crate::tui::widgets::audio::strip_index_at(
            inner(area),
            model.panel_len(panel),
            model.panel_cursor(panel),
            pos,
        ),
        other => {
            let heights = vec![1u16; model.panel_len(other)];
            index_at_row(area, &heights, hits.offsets[other], pos.y)
        }
    }
}

/// Index of the list item drawn at row `y`, or `None` for a click on the
/// border or on empty space past the last item.
///
/// `first` is the item Ratatui actually drew at the top, from the frame that
/// produced these hitboxes — not a re-derivation of where it ought to have
/// scrolled to.
fn index_at_row(area: Rect, heights: &[u16], first: usize, y: u16) -> Option<usize> {
    index_at_content_row(inner(area), heights, first, y)
}

/// [`index_at_row`] for a list whose content rect is already known, which is
/// the case inside the scene-profile modal: its list is one section of the
/// popup's interior, not a bordered panel of its own.
fn index_at_content_row(inner: Rect, heights: &[u16], first: usize, y: u16) -> Option<usize> {
    if inner.height == 0 || y < inner.y || y >= inner.y.saturating_add(inner.height) {
        return None;
    }
    let bottom = inner.y.saturating_add(inner.height);
    let mut row = inner.y;
    for (index, height) in heights.iter().enumerate().skip(first) {
        let next = row.saturating_add(*height);
        if y < next {
            return Some(index);
        }
        row = next;
        if row >= bottom {
            break;
        }
    }
    None
}

/// Map a mouse event onto an action, or `None` when it lands somewhere with
/// nothing to do (motion, drag, empty space).
pub fn handle_mouse(model: &TuiModel, hits: &Hitboxes, event: MouseEvent) -> Option<TuiAction> {
    let pos = Position::new(event.column, event.row);

    match hits.view {
        HitView::Disconnected => match event.kind {
            MouseEventKind::Down(MouseButton::Left) => Some(TuiAction::RetryConnect),
            _ => None,
        },
        HitView::Settings => settings_mouse(model, hits, event, pos),
        // The editor is modal: it answers what it can and swallows the rest.
        // Nothing may fall through to the dashboard it covers, which would
        // switch a scene the user cannot even see.
        HitView::SceneProfile => scene_profile_mouse(model, hits, event, pos),
        HitView::Main => main_mouse(model, hits, event, pos),
    }
}

/// Mouse inside the scene-profile editor: the wheel and a left-click move its
/// cursor, and a right-click is the Esc of whichever stage is up.
///
/// Deliberately no "click the selected row again to act on it" as the settings
/// list has. There the repeated click applies a theme, which is undone by
/// picking another; here the stage's Enter either leaves the picker or writes
/// the config file, and neither belongs on a double-click.
fn scene_profile_mouse(
    model: &TuiModel,
    hits: &Hitboxes,
    event: MouseEvent,
    pos: Position,
) -> Option<TuiAction> {
    let stage = model.scene_profile.as_ref()?.stage;

    // Right-click is the mouse's Esc throughout the UI, and Esc means a
    // different thing on each stage of this modal.
    if matches!(event.kind, MouseEventKind::Down(MouseButton::Right)) {
        return Some(match stage {
            SceneProfileStage::Picker => TuiAction::CloseSceneProfiles,
            SceneProfileStage::Scenes => TuiAction::SceneProfileBack,
            SceneProfileStage::Naming => TuiAction::SceneProfileNameCancel,
        });
    }

    if !contains(hits.scene_profile_list, pos) {
        return None;
    }
    match event.kind {
        MouseEventKind::ScrollUp => Some(TuiAction::SceneProfileNavUp(WHEEL_ROWS)),
        MouseEventKind::ScrollDown => Some(TuiAction::SceneProfileNavDown(WHEEL_ROWS)),
        // Not while a name is being typed: the rows showing through under the
        // naming overlay belong to the stage behind it, and moving that cursor
        // out from under a half-typed name is not what a click there means.
        MouseEventKind::Down(MouseButton::Left) if stage != SceneProfileStage::Naming => {
            let heights = vec![1u16; model.scene_profile_rows().len()];
            let index = index_at_content_row(
                hits.scene_profile_list,
                &heights,
                hits.offsets.scene_profile,
                event.row,
            )?;
            Some(TuiAction::SceneProfileSelect(index))
        }
        _ => None,
    }
}

fn settings_mouse(
    model: &TuiModel,
    hits: &Hitboxes,
    event: MouseEvent,
    pos: Position,
) -> Option<TuiAction> {
    // Right-click is the mouse's Esc throughout the UI.
    if matches!(event.kind, MouseEventKind::Down(MouseButton::Right)) {
        return Some(TuiAction::CloseSettings);
    }
    if !contains(hits.settings_list, pos) {
        return None;
    }
    match event.kind {
        MouseEventKind::ScrollUp => Some(TuiAction::NavUp(WHEEL_ROWS)),
        MouseEventKind::ScrollDown => Some(TuiAction::NavDown(WHEEL_ROWS)),
        MouseEventKind::Down(MouseButton::Left) => {
            let heights = vec![1u16; crate::tui::theme::ALL.len()];
            let index = index_at_row(
                hits.settings_list,
                &heights,
                hits.offsets.settings,
                event.row,
            )?;
            // Clicking the already-previewed theme confirms it, so a
            // double-click reads as "preview, then apply".
            if index == model.settings_cursor {
                Some(TuiAction::ApplySettingsTheme)
            } else {
                Some(TuiAction::SettingsSelect(index))
            }
        }
        _ => None,
    }
}

fn main_mouse(
    model: &TuiModel,
    hits: &Hitboxes,
    event: MouseEvent,
    pos: Position,
) -> Option<TuiAction> {
    if matches!(event.kind, MouseEventKind::Down(MouseButton::Right)) {
        return Some(if model.command_palette.active {
            TuiAction::ClosePalette
        } else {
            TuiAction::ClearPending
        });
    }

    // The which-key popup floats over the panels, so a click while it is up
    // dismisses it rather than reaching through to whatever it covers.
    if model.pending.is_active() && matches!(event.kind, MouseEventKind::Down(_)) {
        return Some(TuiAction::ClearPending);
    }

    if contains(hits.logs, pos) {
        return match event.kind {
            MouseEventKind::ScrollUp => Some(TuiAction::LogScrollUp(WHEEL_ROWS)),
            MouseEventKind::ScrollDown => Some(TuiAction::LogScrollDown(WHEEL_ROWS)),
            _ => None,
        };
    }

    if contains(hits.palette, pos) {
        return match event.kind {
            MouseEventKind::Down(MouseButton::Left) if !model.command_palette.active => {
                Some(TuiAction::OpenPalette {
                    prefix: None,
                    seed: "",
                })
            }
            MouseEventKind::ScrollDown if model.command_palette.active => {
                Some(TuiAction::CompleteNext)
            }
            MouseEventKind::ScrollUp if model.command_palette.active => {
                Some(TuiAction::CompletePrev)
            }
            _ => None,
        };
    }

    let (panel, area) = hits.panel_at(pos)?;
    let cursor = model.panel_cursor(panel);
    match event.kind {
        MouseEventKind::ScrollUp => Some(TuiAction::SelectIndex(
            panel,
            cursor.saturating_sub(WHEEL_ROWS),
        )),
        MouseEventKind::ScrollDown => Some(TuiAction::SelectIndex(
            panel,
            cursor.saturating_add(WHEEL_ROWS),
        )),
        MouseEventKind::Down(MouseButton::Left) => {
            let index = index_at(model, hits, panel, area, pos)?;
            // First click focuses and selects; clicking the row that is
            // already selected in the already-focused panel activates it.
            if model.focus == panel && index == cursor {
                Some(TuiAction::ActivateIndex(panel, index))
            } else {
                Some(TuiAction::SelectIndex(panel, index))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::state::{AudioState, ObsSnapshot, SceneState};

    fn hits_main() -> Hitboxes {
        Hitboxes {
            view: HitView::Main,
            offsets: ListOffsets::default(),
            scenes: Rect::new(0, 0, 40, 12),
            audio: Rect::new(40, 0, 40, 12),
            profiles: Rect::new(0, 12, 40, 7),
            collections: Rect::new(40, 12, 40, 7),
            logs: Rect::new(0, 19, 80, 7),
            palette: Rect::new(0, 26, 80, 4),
            settings_list: Rect::default(),
            scene_profile_list: Rect::default(),
        }
    }

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    fn scene(name: &str) -> SceneState {
        SceneState {
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn model_with_scenes(count: usize) -> TuiModel {
        let mut model = TuiModel::default();
        model.set_snapshot(ObsSnapshot {
            scenes: (0..count).map(|i| scene(&format!("scene{i}"))).collect(),
            ..Default::default()
        });
        model.clamp_cursors();
        model
    }

    #[test]
    fn clicking_a_row_focuses_its_panel_and_selects_that_row() {
        let model = model_with_scenes(5);
        // Panel starts at y=0; the border eats row 0, so row 3 is item 2.
        let action = handle_mouse(&model, &hits_main(), click(5, 3));
        assert_eq!(action, Some(TuiAction::SelectIndex(FocusPanel::Scenes, 2)));
    }

    #[test]
    fn clicking_the_already_selected_row_of_the_focused_panel_activates_it() {
        let mut model = model_with_scenes(5);
        model.focus = FocusPanel::Scenes;
        model.set_panel_cursor(FocusPanel::Scenes, 2);
        let action = handle_mouse(&model, &hits_main(), click(5, 3));
        assert_eq!(
            action,
            Some(TuiAction::ActivateIndex(FocusPanel::Scenes, 2))
        );

        // The same row in an unfocused panel only selects.
        model.focus = FocusPanel::Audio;
        let action = handle_mouse(&model, &hits_main(), click(5, 3));
        assert_eq!(action, Some(TuiAction::SelectIndex(FocusPanel::Scenes, 2)));
    }

    #[test]
    fn clicking_a_panel_border_or_past_the_last_row_does_nothing() {
        let model = model_with_scenes(2);
        assert_eq!(handle_mouse(&model, &hits_main(), click(5, 0)), None);
        // Only two items, so row 5 is empty space inside the panel.
        assert_eq!(handle_mouse(&model, &hits_main(), click(5, 5)), None);
    }

    #[test]
    fn audio_clicks_resolve_by_column_because_the_strips_are_vertical() {
        let mut model = TuiModel::default();
        model.set_snapshot(ObsSnapshot {
            audio_inputs: vec![
                AudioState {
                    name: "Mic".into(),
                    ..Default::default()
                },
                AudioState {
                    name: "Desktop".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        model.clamp_cursors();

        // The audio pane is 40 columns starting at x=40, so its interior is
        // 41..=78. Two strips cap out at 16 columns each and the pair is
        // centered: 43..=58, a gap at 59, then 60..=75.
        assert_eq!(
            handle_mouse(&model, &hits_main(), click(43, 1)),
            Some(TuiAction::SelectIndex(FocusPanel::Audio, 0))
        );
        // A click low down the same strip is still that input — the whole
        // column belongs to it, whatever row was hit.
        assert_eq!(
            handle_mouse(&model, &hits_main(), click(50, 9)),
            Some(TuiAction::SelectIndex(FocusPanel::Audio, 0))
        );
        assert_eq!(
            handle_mouse(&model, &hits_main(), click(62, 4)),
            Some(TuiAction::SelectIndex(FocusPanel::Audio, 1))
        );
        // Column 59 is the blank gap between the two strips.
        assert_eq!(handle_mouse(&model, &hits_main(), click(59, 4)), None);
        // Left of the first strip and right of the last there is nothing.
        assert_eq!(handle_mouse(&model, &hits_main(), click(41, 4)), None);
        assert_eq!(handle_mouse(&model, &hits_main(), click(77, 4)), None);
    }

    /// A click resolves against the window the last frame actually drew, which
    /// the hitboxes carry, rather than against a guess recomputed from the
    /// cursor.
    #[test]
    fn clicks_below_the_fold_resolve_against_the_scrolled_window() {
        let mut model = model_with_scenes(40);
        model.set_panel_cursor(FocusPanel::Scenes, 39);

        let hits = Hitboxes {
            offsets: ListOffsets {
                scenes: 30,
                ..ListOffsets::default()
            },
            ..hits_main()
        };

        let action = handle_mouse(&model, &hits, click(5, 1));
        assert_eq!(action, Some(TuiAction::SelectIndex(FocusPanel::Scenes, 30)));
    }

    #[test]
    fn the_wheel_moves_the_cursor_within_the_hovered_panel() {
        let mut model = model_with_scenes(20);
        model.set_panel_cursor(FocusPanel::Scenes, 10);
        assert_eq!(
            handle_mouse(
                &model,
                &hits_main(),
                wheel(MouseEventKind::ScrollDown, 5, 4)
            ),
            Some(TuiAction::SelectIndex(FocusPanel::Scenes, 13))
        );
        assert_eq!(
            handle_mouse(&model, &hits_main(), wheel(MouseEventKind::ScrollUp, 5, 4)),
            Some(TuiAction::SelectIndex(FocusPanel::Scenes, 7))
        );
    }

    #[test]
    fn the_wheel_over_the_logs_pane_scrolls_history_instead() {
        let model = model_with_scenes(3);
        assert_eq!(
            handle_mouse(
                &model,
                &hits_main(),
                wheel(MouseEventKind::ScrollUp, 10, 21)
            ),
            Some(TuiAction::LogScrollUp(WHEEL_ROWS))
        );
        assert_eq!(
            handle_mouse(
                &model,
                &hits_main(),
                wheel(MouseEventKind::ScrollDown, 10, 21)
            ),
            Some(TuiAction::LogScrollDown(WHEEL_ROWS))
        );
    }

    #[test]
    fn clicking_the_command_bar_opens_the_palette_at_the_configured_prefix() {
        let mut model = model_with_scenes(3);
        assert_eq!(
            handle_mouse(&model, &hits_main(), click(10, 27)),
            Some(TuiAction::OpenPalette {
                prefix: None,
                seed: ""
            })
        );
        // Already open: a click there does not reopen and wipe the input.
        model.command_palette.active = true;
        assert_eq!(handle_mouse(&model, &hits_main(), click(10, 27)), None);
    }

    #[test]
    fn right_click_cancels_like_escape() {
        let mut model = model_with_scenes(3);
        let right = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 5,
            row: 3,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        assert_eq!(
            handle_mouse(&model, &hits_main(), right),
            Some(TuiAction::ClearPending)
        );
        model.command_palette.active = true;
        assert_eq!(
            handle_mouse(&model, &hits_main(), right),
            Some(TuiAction::ClosePalette)
        );
    }

    #[test]
    fn motion_and_drag_events_are_ignored() {
        let model = model_with_scenes(5);
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            assert_eq!(handle_mouse(&model, &hits_main(), wheel(kind, 5, 3)), None);
        }
    }

    #[test]
    fn the_disconnected_screen_retries_on_any_click() {
        let model = TuiModel::default();
        let hits = Hitboxes {
            view: HitView::Disconnected,
            ..Default::default()
        };
        assert_eq!(
            handle_mouse(&model, &hits, click(10, 10)),
            Some(TuiAction::RetryConnect)
        );
        assert_eq!(
            handle_mouse(&model, &hits, wheel(MouseEventKind::ScrollUp, 10, 10)),
            None
        );
    }

    #[test]
    fn settings_rows_preview_on_click_and_apply_on_a_second_click() {
        let mut model = TuiModel::default();
        model.settings_cursor = 0;
        let hits = Hitboxes {
            view: HitView::Settings,
            settings_list: Rect::new(0, 0, 30, 20),
            ..Default::default()
        };

        assert_eq!(
            handle_mouse(&model, &hits, click(5, 4)),
            Some(TuiAction::SettingsSelect(3))
        );
        model.settings_cursor = 3;
        assert_eq!(
            handle_mouse(&model, &hits, click(5, 4)),
            Some(TuiAction::ApplySettingsTheme)
        );
    }
}
