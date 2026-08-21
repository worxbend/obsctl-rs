//! The shared body of the Profiles and Collections panels.
//!
//! Both show the same thing: a numbered list of names OBS knows, with the one
//! currently in use marked and bolded, and Enter switching to the highlighted
//! row. They were two files that differed only in which names they listed and
//! what the panel was called, so the rendering lives here once and each panel
//! supplies its own [`NameListPanel`].

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState},
};

use rust_i18n::t;

use crate::tui::{
    model::{FocusPanel, TuiModel},
    widgets::chrome,
};

/// What tells one name-list panel apart from another.
pub struct NameListPanel {
    /// Which panel this is, for focus and cursor lookups.
    pub panel: FocusPanel,
    /// The names to list.
    pub names: fn(&TuiModel) -> &[String],
    /// The one of `names` OBS is currently using, if any.
    pub current: fn(&TuiModel) -> Option<&str>,
    /// Panel icon, as `(rich, ascii)` — the model picks per `rich_ui`.
    pub icon: (&'static str, &'static str),
    /// Translation key for the panel title. A key rather than the text
    /// itself because these panels are built as `const`s, which cannot call
    /// `t!` — so the lookup is deferred to render time.
    pub title_key: &'static str,
    /// Translation keys for the key hint in the panel's top-right corner, as
    /// `(rich, ascii)`. The model picks per `rich_ui`, the same way the icon
    /// above it does.
    pub hint_keys: (&'static str, &'static str),
}

/// Returns the scroll offset Ratatui settled on, so the mouse code can map a
/// click to a row without re-deriving it. See [`crate::tui::mouse`].
#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel, spec: &NameListPanel) -> usize {
    let theme = model.theme;
    let focused = model.focus == spec.panel;
    let names = (spec.names)(model);
    let current = (spec.current)(model);

    let items: Vec<ListItem> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let active = Some(name.as_str()) == current;
            let marker = if active {
                model.symbol("▶", ">")
            } else {
                model.symbol("◇", " ")
            };
            let style = if active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {:02} ", index + 1),
                    Style::default().fg(theme.muted),
                ),
                Span::styled(
                    format!("{marker} "),
                    Style::default().fg(if active { theme.success } else { theme.muted }),
                ),
                Span::styled(name.as_str(), Style::default().fg(theme.fg)),
            ]))
            .style(style)
        })
        .collect();

    let title = t!(spec.title_key);
    let hint = t!(model.symbol(spec.hint_keys.0, spec.hint_keys.1));
    let block = chrome::panel(
        model.symbol(spec.icon.0, spec.icon.1),
        &title,
        &hint,
        names.len(),
        focused,
        model,
    );

    let mut state = ListState::default();
    if !names.is_empty() {
        state.select(Some(model.panel_cursor(spec.panel)));
    }

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(theme.selection_style(focused)),
        area,
        &mut state,
    );
    state.offset()
}
