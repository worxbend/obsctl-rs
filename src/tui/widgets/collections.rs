use ratatui::{Frame, layout::Rect};

use crate::tui::{
    model::{FocusPanel, TuiModel},
    widgets::name_list::{self, NameListPanel},
};

const COLLECTIONS: NameListPanel = NameListPanel {
    panel: FocusPanel::Collections,
    names: TuiModel::scene_collections,
    current: TuiModel::current_scene_collection,
    icon: ("🗂", "C"),
    title_key: "tui.panels.collections.title",
    hint_keys: (
        "tui.panels.collections.hint",
        "tui.panels.collections.hint_ascii",
    ),
};

#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) -> usize {
    name_list::render(f, area, model, &COLLECTIONS)
}
