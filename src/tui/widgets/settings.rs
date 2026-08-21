use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph},
};

use rust_i18n::t;

use crate::tui::{anim, model::TuiModel, theme, theme::Theme, widgets::chrome};

/// Full-screen settings view — currently just the theme picker, styled
/// after btop's theme switcher: arrow keys live-preview a theme across the
/// whole UI, Enter confirms and persists it, Esc reverts to whatever was
/// active before opening this view.
/// Draws the settings view and returns the theme list's area together with the
/// scroll offset Ratatui settled on, for the mouse code to map a click to a
/// row. See [`crate::tui::mouse`].
#[must_use]
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) -> (Rect, usize) {
    let theme = model.theme;

    let title_text = t!(chrome::glyph(
        model,
        "tui.panels.settings.title",
        "tui.panels.settings.title_ascii"
    ))
    .into_owned();
    let title = if model.advanced_ui {
        anim::gradient_line(
            &title_text,
            theme.accent,
            theme.accent_alt,
            model.anim.frame,
            true,
        )
    } else {
        Line::styled(
            title_text,
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
    };
    let outer = chrome::ascii_aware(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .border_style(Style::default().fg(theme.border_focus))
            .title(title),
        model,
    );
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let sections = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(inner);

    let offset = render_theme_list(f, sections[0], model);
    render_preview(f, sections[1], model);
    (sections[0], offset)
}

#[must_use]
fn render_theme_list(f: &mut Frame, area: Rect, model: &TuiModel) -> usize {
    let theme = model.theme;
    let items: Vec<ListItem> = theme::ALL
        .iter()
        .map(|t| {
            let cell = chrome::glyph(model, "██", "##");
            let swatch = Line::from(vec![
                Span::styled(cell, Style::default().fg(t.accent)),
                Span::styled(cell, Style::default().fg(t.success)),
                Span::styled(cell, Style::default().fg(t.warning)),
                Span::styled(cell, Style::default().fg(t.danger)),
                Span::raw("  "),
                Span::raw(t.label),
            ]);
            ListItem::new(swatch)
        })
        .collect();

    let block = chrome::bordered(
        model,
        BorderType::Rounded,
        theme.border,
        t!(
            "tui.panels.settings.themes",
            count = format!("{:02}", theme::ALL.len())
        )
        .into_owned(),
    );

    let highlight_style = Style::default()
        .bg(theme.highlight_bg)
        .fg(theme.highlight_fg)
        .add_modifier(Modifier::BOLD);

    let mut state = ListState::default();
    state.select(Some(model.settings_cursor));

    f.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(highlight_style),
        area,
        &mut state,
    );
    state.offset()
}

/// One coloured run inside a preview row: how it is spelled with the full
/// glyph set, how it is spelled in ASCII, and the theme style that paints it.
///
/// The preview used to be two hand-maintained screens sitting in the two arms
/// of an `if` — a twelve-line Unicode `vec!` and an eleven-line ASCII one —
/// so adding a single row meant editing both and keeping them lined up.
/// Pairing the two spellings run by run means a row is written once.
type PreviewRun = (&'static str, &'static str, fn(Theme) -> Style);

/// The body of the preview, row by row, below its animated heading.
///
/// An empty row is a blank line. A run whose ASCII spelling is empty draws
/// nothing at all, which is how the four-colour meter bar collapses into the
/// single-colour ASCII one without needing a second table.
const PREVIEW_ROWS: &[&[PreviewRun]] = &[
    &[],
    &[
        ("● LIVE", "* LIVE", |t| Style::default().fg(t.danger)),
        ("   ◉ REC", "   * REC", |t| Style::default().fg(t.warning)),
        ("   ◆ SCENE ACTIVE", "   > SCENE ACTIVE", |t| {
            Style::default().fg(t.accent)
        }),
    ],
    &[("✓ connected", "+ connected", |t| {
        Style::default().fg(t.success)
    })],
    &[("⚠ warning", "! warning", |t| {
        Style::default().fg(t.warning)
    })],
    &[("ℹ info", "i info", |t| Style::default().fg(t.info))],
    &[
        ("▰▰▰▰", "#######...  -12.4 dB", |t| {
            Style::default().fg(t.success)
        }),
        ("▰▰▰", "", |t| Style::default().fg(t.warning)),
        ("▰▰", "", |t| Style::default().fg(t.danger)),
        ("▱▱▱  -12.4 dB", "", |t| Style::default().fg(t.muted)),
    ],
    &[(
        "CPU  ▁▂▃▅▄▆▇█▆▄   NET  ▁▁▂▃▅▇▅▃",
        "CPU  .-=+*#*+   NET  ..-=*#+-",
        |t| Style::default().fg(t.info),
    )],
    &[("muted text", "muted text", |t| Style::default().fg(t.muted))],
    &[],
    &[(" selected row ", " selected row ", |t| {
        Style::default().bg(t.highlight_bg).fg(t.highlight_fg)
    })],
];

fn render_preview(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let block = chrome::bordered(
        model,
        BorderType::Rounded,
        theme.border_focus,
        t!("tui.panels.settings.preview", theme = theme.label).into_owned(),
    );
    let Some(inner) = chrome::frame(f, area, block) else {
        return;
    };

    // The heading is the one row the table cannot describe: with the advanced
    // UI on it is an animated colour wave rather than a single style.
    let heading_text = t!(chrome::glyph(
        model,
        "tui.panels.settings.preview_heading",
        "tui.panels.settings.preview_heading_ascii"
    ))
    .into_owned();
    let mut lines = vec![if model.advanced_ui {
        anim::gradient_line(
            &heading_text,
            theme.accent,
            theme.accent_alt,
            model.anim.frame,
            true,
        )
    } else {
        Line::styled(heading_text, Style::default().fg(theme.accent))
    }];
    lines.extend(PREVIEW_ROWS.iter().map(|row| {
        Line::from(
            row.iter()
                .map(|(rich, plain, style)| {
                    Span::styled(chrome::glyph(model, rich, plain), style(theme))
                })
                .collect::<Vec<_>>(),
        )
    }));

    let paragraph = Paragraph::new(lines).alignment(Alignment::Left);
    f.render_widget(paragraph, inner);
}
