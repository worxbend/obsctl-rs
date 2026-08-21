use ratatui::{Frame, layout::Rect};

use crate::tui::{
    model::{FocusPanel, TuiModel},
    widgets::name_list::{self, NameListPanel},
};

const PROFILES: NameListPanel = NameListPanel {
    panel: FocusPanel::Profiles,
    names: TuiModel::profiles,
    current: TuiModel::current_profile,
    icon: ("🎛", "P"),
    title_key: "tui.panels.profiles.title",
    hint_keys: ("tui.panels.profiles.hint", "tui.panels.profiles.hint_ascii"),
};

#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) -> usize {
    name_list::render(f, area, model, &PROFILES)
}
