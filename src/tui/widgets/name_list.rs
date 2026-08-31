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
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState},
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
    /// Panel icon, as `(rich, ascii)` — a pictogram pair, read with
    /// [`TuiModel::symbol`], so it follows `rich_ui` (icons *and* Unicode).
    pub icon: (&'static str, &'static str),
    /// Translation key for the panel title. A key rather than the text
    /// itself because these panels are built as `const`s, which cannot call
    /// `t!` — so the lookup is deferred to render time.
    pub title_key: &'static str,
    /// Translation keys for the key hint in the panel's top-right corner, as
    /// `(advanced, plain)`. Read with [`chrome::phrase`], so unlike the icon
    /// above it this pair follows `advanced_ui` alone: the hint's arrows are
    /// box-drawing-class characters, not emoji, and a terminal that can draw
    /// them should get them even with icons switched off.
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
            let style = if active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let mut spans = Vec::from(row_prefix(model, index, active, theme.success));
            spans.push(Span::styled(name.as_str(), Style::default().fg(theme.fg)));
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let title = t!(spec.title_key);
    let hint = chrome::phrase(model, spec.hint_keys.0, spec.hint_keys.1);
    let block = chrome::panel(
        model.symbol(spec.icon.0, spec.icon.1),
        &title,
        &hint,
        names.len(),
        focused,
        model,
    );

    render_rows(f, area, model, spec.panel, block, items)
}

/// The two spans every list row opens with: the muted position badge and the
/// marker saying whether this row is the one OBS is currently using.
///
/// `index` is the row's zero-based position; the badge shows it one-based, the
/// way the number keys that jump to it are spelled. `active_color` is the
/// marker's colour when the row is active — the scenes panel tints it while a
/// freshly switched scene flashes, everything else passes the plain success
/// colour.
pub fn row_prefix(
    model: &TuiModel,
    index: usize,
    active: bool,
    active_color: Color,
) -> [Span<'static>; 2] {
    let theme = model.theme;
    let marker = if active {
        model.symbol("▶", ">")
    } else {
        model.symbol("◇", " ")
    };
    [
        Span::styled(
            format!(" {:02} ", index + 1),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            format!("{marker} "),
            Style::default().fg(if active { active_color } else { theme.muted }),
        ),
    ]
}

/// Draw `items` into `area` as the panel's list, and hand back the scroll
/// offset Ratatui settled on so the mouse code can map a click to a row.
///
/// The selection is only set when there is something to select: an empty list
/// with a selected index draws a highlight bar over nothing.
#[must_use]
pub fn render_rows<'a>(
    f: &mut Frame,
    area: Rect,
    model: &TuiModel,
    panel: FocusPanel,
    block: Block<'a>,
    items: Vec<ListItem<'a>>,
) -> usize {
    let focused = model.focus == panel;
    let mut state = ListState::default();
    if !items.is_empty() {
        state.select(Some(model.panel_cursor(panel)));
    }

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(model.theme.selection_style(focused)),
        area,
        &mut state,
    );
    state.offset()
}
