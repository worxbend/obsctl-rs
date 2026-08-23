//! Frame composition: which widget is drawn where, and where the frame put
//! the regions a mouse event has to resolve against.
//!
//! The widgets themselves live in [`crate::tui::widgets`]; this module only
//! decides the order they are drawn in and hands back the [`Hitboxes`] the
//! next mouse event is hit-tested with.

use ratatui::layout::Rect;

use crate::tui::{
    layout,
    model::{FocusPanel, TuiModel, View},
    mouse::{HitView, Hitboxes, ListOffsets},
    widgets,
};

/// Draw a frame, returning where the interactive regions ended up so mouse
/// events can be resolved against the pixels the user is actually looking at.
pub(super) fn render(f: &mut ratatui::Frame, model: &TuiModel) -> Hitboxes {
    widgets::fill_background(f, model.theme);

    if model.view == View::Settings {
        let (settings_list, settings_offset) = widgets::settings::render(f, f.area(), model);
        return Hitboxes {
            view: HitView::Settings,
            settings_list,
            offsets: ListOffsets {
                settings: settings_offset,
                ..ListOffsets::default()
            },
            ..Hitboxes::default()
        };
    }

    let areas = layout::compute(f, model.streaming());

    if !model.connected_to_daemon {
        widgets::connection::render_unavailable(f, f.area(), model);
        // The editor can be opened before the daemon goes away, and its keys
        // are swallowed for as long as it is open — so it has to stay on
        // screen here too, rather than leaving the user typing at something
        // invisible.
        return overlay_scene_profile(
            f,
            model,
            Hitboxes {
                view: HitView::Disconnected,
                ..Hitboxes::default()
            },
        );
    }

    widgets::header::render(f, areas.header, model);
    // The live bar only draws its own state badges when the big top-right
    // pane is not on screen, so the state is never spelled out twice.
    widgets::live_bar::render(f, areas.live_bar, model, areas.status.is_none());
    if let Some(status_area) = areas.status {
        widgets::status::render(f, status_area, model);
    }
    // The lists report the scroll offset Ratatui gave them, which is what the
    // mouse code maps a click row through.
    let scenes_offset = widgets::scenes::render(f, areas.scenes, model);
    widgets::audio::render(f, areas.audio, model);
    let profiles_offset = widgets::profiles::render(f, areas.profiles, model);
    let collections_offset = widgets::collections::render(f, areas.collections, model);
    widgets::logs::render(f, areas.logs, model);
    if let Some(stats_area) = areas.stats {
        widgets::stats::render(f, stats_area, model);
    }
    widgets::command_palette::render(f, areas.palette, model);
    // Last, so the which-key popup floats above the dashboard.
    widgets::which_key::render(f, f.area(), model);

    let hits = Hitboxes {
        view: HitView::Main,
        scenes: areas.scenes,
        audio: areas.audio,
        profiles: areas.profiles,
        collections: areas.collections,
        logs: areas.logs,
        palette: areas.palette,
        settings_list: Rect::default(),
        scene_profile_list: Rect::default(),
        offsets: ListOffsets {
            scenes: scenes_offset,
            profiles: profiles_offset,
            collections: collections_offset,
            settings: 0,
            scene_profile: 0,
        },
    };

    overlay_scene_profile(f, model, hits)
}

/// Draw the scene-profile editor on top of whatever the frame already holds,
/// and say so in the hitboxes when it is up.
///
/// It goes on last — above the which-key popup as well — because it is modal:
/// it owns the keyboard while it is open, and the panel rects it covers must
/// stop answering clicks even though the frame still drew them. The rects
/// themselves are left alone so they are ready again the moment it closes.
fn overlay_scene_profile(f: &mut ratatui::Frame, model: &TuiModel, hits: Hitboxes) -> Hitboxes {
    let (scene_profile_list, offset) = widgets::scene_profile::render(f, f.area(), model);
    if model.scene_profile.is_some() {
        Hitboxes {
            view: HitView::SceneProfile,
            scene_profile_list,
            offsets: ListOffsets {
                scene_profile: offset,
                ..hits.offsets
            },
            ..hits
        }
    } else {
        hits
    }
}

/// How far `Ctrl-D`/`Ctrl-U` (and `PgDn`/`PgUp`) move — half a screenful of
/// whatever the current screen scrolls, and at least one step before the
/// first frame has been drawn and the hitboxes are still empty.
///
/// The list panels scroll rows; the audio matrix scrolls channel strips
/// sideways, so half a page there is half the strips that fit across it, not
/// half its height; and in the settings view the thing that moves is the
/// theme list rather than any panel.
pub(super) fn half_page(hits: &Hitboxes, model: &TuiModel) -> usize {
    /// Rows inside a one-cell block border.
    fn rows_in(area: Rect) -> usize {
        usize::from(area.height.saturating_sub(2))
    }

    let items = match (&model.view, model.focus) {
        (View::Settings, _) => rows_in(hits.settings_list),
        (View::Main, FocusPanel::Audio) => {
            widgets::audio::visible_strips(hits[FocusPanel::Audio].width.saturating_sub(2))
        }
        (View::Main, panel) => rows_in(hits[panel]),
    };
    (items / 2).max(1)
}
