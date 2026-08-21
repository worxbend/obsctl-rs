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
    title: "Collections",
    hint: ("[c]  ↵ switch", "[c]  Enter switch"),
};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    name_list::render(f, area, model, &COLLECTIONS);
}
