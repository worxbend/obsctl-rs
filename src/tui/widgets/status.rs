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

/// Columns between two block-font words when both LIVE and REC are showing.
const WORD_GAP: usize = 2;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let states = spinner::active_states(model.streaming(), model.recording());
    let broadcasting = states[0] != BroadcastState::Idle;
    let pulse = model.anim.pulse(anim::PULSE_PERIOD_TICKS);

    let border = if broadcasting {
        // A live pane starts from a tint that is already visible and breathes
        // on top of it, which the shared 0..amount ramp cannot express.
        if model.advanced_ui {
            anim::blend(theme.border, theme.danger, 0.35 + pulse * 0.55)
        } else {
            theme.border
        }
    } else {
        chrome::breathing_border(model, theme.border, theme.info, 0.25)
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
        vec![state_line(&states, model, pulse, StateLineForm::Standalone)]
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
    lines.push(state_line(
        states,
        model,
        pulse,
        StateLineForm::UnderBlockWord,
    ));
    lines
}

/// Where a [`state_line`] is going, which decides how it is punctuated.
enum StateLineForm {
    /// Sitting under the block-font word, which has already said `LIVE`/`REC`:
    /// the label is left out and entries are divided by the shared rule.
    UnderBlockWord,
    /// The whole pane, for sizes too small for the block font: the label is
    /// spelled out and entries are only spaced apart.
    Standalone,
}

/// Spinner + elapsed time for each active state, centred on one row.
///
/// The two forms were two functions differing in exactly the two things named
/// on [`StateLineForm`], which is a lot of duplicated span-building to keep in
/// step for a divider and a word.
fn state_line(
    states: &[BroadcastState],
    model: &TuiModel,
    pulse: f32,
    form: StateLineForm,
) -> Line<'static> {
    let theme = model.theme;
    let mut spans = Vec::with_capacity(states.len() * 2);
    for (index, state) in states.iter().enumerate() {
        if index > 0 {
            spans.push(match form {
                StateLineForm::UnderBlockWord => chrome::separator(model),
                StateLineForm::Standalone => Span::raw(" "),
            });
        }
        let text = match form {
            StateLineForm::UnderBlockWord => format!(
                "{} {}",
                spinner::frame(*state, model.rich_ui(), model.anim.frame),
                detail_text(*state, model)
            ),
            StateLineForm::Standalone => format!(
                " {} {} {} ",
                spinner::frame(*state, model.rich_ui(), model.anim.frame),
                state.label(),
                detail_text(*state, model)
            ),
        };
        spans.push(Span::styled(
            text,
            Style::default()
                .fg(color(*state, theme, pulse))
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans).alignment(Alignment::Center)
}

fn detail_text(state: BroadcastState, model: &TuiModel) -> String {
    chrome::broadcast_detail(state, model, t!("tui.status.idle_hint"))
}

/// Distinct hues per state, so LIVE and REC stay tellable apart at a glance
/// even when both are showing.
fn color(state: BroadcastState, theme: Theme, pulse: f32) -> Color {
    chrome::broadcast_pulse_color(state, theme, pulse, 0.4)
        .unwrap_or_else(|| anim::blend(theme.muted, theme.info, pulse * 0.45))
}
