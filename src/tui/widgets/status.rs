//! Oversized broadcast-state pane pinned to the top-right corner.
//!
//! Shows `IDLE` when neither the stream nor the recording is running, and
//! swaps in `LIVE` / `REC` (or both, side by side) as they start. Each state
//! carries its own `rattles` spinner — see `tui::spinner` — so the state is
//! readable from the animation alone.
//!
//! The pane is only laid out when the terminal is wide enough (see
//! `layout::STATUS_PANE_WIDTH`); when it is not, `live_bar` falls back to its
//! inline badges so the state is never hidden entirely.

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
};
use rust_i18n::t;

use crate::tui::{
    anim,
    model::TuiModel,
    spinner::{self, BroadcastState},
    theme::Theme,
    widgets::chrome,
};

const PULSE_PERIOD_TICKS: u64 = 24;

/// Columns between two block-font words when both LIVE and REC are showing.
const WORD_GAP: usize = 2;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let states = spinner::active_states(model.streaming(), model.recording());
    let broadcasting = states[0] != BroadcastState::Idle;
    let pulse = model.anim.pulse(PULSE_PERIOD_TICKS);

    let border = if !model.advanced_ui {
        theme.border
    } else if broadcasting {
        anim::blend(theme.border, theme.danger, 0.35 + pulse * 0.55)
    } else {
        anim::blend(theme.border, theme.info, pulse * 0.25)
    };
    let title_icon = model.symbol("◉", "*");
    let block = chrome::bordered(
        model,
        BorderType::Rounded,
        border,
        Line::styled(
            format!(" {title_icon} {} ", t!("tui.status.title")),
            Style::default()
                .fg(if broadcasting {
                    theme.danger
                } else {
                    theme.muted
                })
                .add_modifier(Modifier::BOLD),
        ),
    );
    let Some(inner) = chrome::frame(f, area, block) else {
        return;
    };

    let mut lines = if fits_block_form(&states, inner) {
        block_form(&states, model, pulse)
    } else {
        vec![compact_line(&states, model, pulse)]
    };

    // Center vertically so the pane reads as one bold object rather than
    // text pinned under the title.
    let padding = (inner.height as usize).saturating_sub(lines.len()) / 2;
    for _ in 0..padding {
        lines.insert(0, Line::raw(""));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// The big form needs three rows of block text plus a detail row, and enough
/// width for every active word. Otherwise the pane degrades to one line.
fn fits_block_form(states: &[BroadcastState], inner: Rect) -> bool {
    inner.height as usize > spinner::BLOCK_HEIGHT && block_row_width(states) <= inner.width as usize
}

fn block_row_width(states: &[BroadcastState]) -> usize {
    let words: usize = states
        .iter()
        .map(|state| spinner::block_width(state.label()))
        .sum();
    words + WORD_GAP * states.len().saturating_sub(1)
}

fn block_form(states: &[BroadcastState], model: &TuiModel, pulse: f32) -> Vec<Line<'static>> {
    // A solid block is box drawing, not a pictogram, so it follows
    // `advanced_ui` like the settings swatches and the stats budget bar do.
    // This used to read `rich_ui()`, which also demands `show_icons`, so a
    // user who had turned icons off but kept Unicode on saw these letters
    // built out of `#` while every other block glyph on screen stayed `█`.
    let fill = chrome::glyph_char(model, '█', '#');

    // Words are laid out column-wise (one word per state, side by side) but
    // rendered row-wise, so fan each word's rows out into the shared row
    // buffers as we go.
    let mut rows: [Vec<Span<'static>>; spinner::BLOCK_HEIGHT] =
        std::array::from_fn(|_| Vec::with_capacity(states.len() * 2));
    for (index, state) in states.iter().enumerate() {
        let style = Style::default()
            .fg(color(*state, model.theme, pulse))
            .add_modifier(Modifier::BOLD);
        for (row, cells) in rows
            .iter_mut()
            .zip(spinner::block_word(state.label(), fill))
        {
            if index > 0 {
                row.push(Span::raw(" ".repeat(WORD_GAP)));
            }
            row.push(Span::styled(cells, style));
        }
    }

    let mut lines: Vec<Line<'static>> = rows
        .into_iter()
        .map(|spans| Line::from(spans).alignment(Alignment::Center))
        .collect();
    lines.push(detail_line(states, model, pulse));
    lines
}

/// Spinner + elapsed time under the block word, one entry per active state.
fn detail_line(states: &[BroadcastState], model: &TuiModel, pulse: f32) -> Line<'static> {
    let theme = model.theme;
    let mut spans = Vec::with_capacity(states.len() * 2);
    for (index, state) in states.iter().enumerate() {
        if index > 0 {
            spans.push(chrome::separator(model));
        }
        spans.push(Span::styled(
            format!(
                "{} {}",
                spinner::frame(*state, model.rich_ui(), model.anim.frame),
                detail_text(*state, model)
            ),
            Style::default()
                .fg(color(*state, theme, pulse))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans).alignment(Alignment::Center)
}

/// Single-line fallback for panes too small for the block font.
fn compact_line(states: &[BroadcastState], model: &TuiModel, pulse: f32) -> Line<'static> {
    let theme = model.theme;
    let mut spans = Vec::with_capacity(states.len() * 2);
    for (index, state) in states.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            format!(
                " {} {} {} ",
                spinner::frame(*state, model.rich_ui(), model.anim.frame),
                state.label(),
                detail_text(*state, model)
            ),
            Style::default()
                .fg(color(*state, theme, pulse))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans).alignment(Alignment::Center)
}

fn detail_text(state: BroadcastState, model: &TuiModel) -> String {
    match state {
        BroadcastState::Idle => t!("tui.status.idle_hint").into_owned(),
        BroadcastState::Live => chrome::format_duration(model.stream_duration_ms()),
        BroadcastState::Rec => chrome::format_duration(model.record_duration_ms()),
    }
}

/// Distinct hues per state, so LIVE and REC stay tellable apart at a glance
/// even when both are showing.
fn color(state: BroadcastState, theme: Theme, pulse: f32) -> Color {
    match state {
        BroadcastState::Idle => anim::blend(theme.muted, theme.info, pulse * 0.45),
        BroadcastState::Live => anim::blend(theme.danger, theme.warning, pulse * 0.4),
        BroadcastState::Rec => anim::blend(theme.warning, theme.danger, pulse * 0.4),
    }
}
