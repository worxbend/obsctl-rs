//! The audio matrix, laid out the way OBS Studio's Audio Mixer is.
//!
//! Every input gets its own bordered **vertical channel strip**, and the
//! strips sit side by side with a blank column between them. A strip reads,
//! top to bottom:
//!
//! ```text
//! ╭── Mic ──╮  input name (accented when it is the selected strip)
//! │ mic [1] │  alias and keyboard shortcut, when the strip is tall enough
//! │ -3.0 dB │  the fader's gain, the same number OBS prints under the name
//! │▐ ███   0│  fader ┃ meter ┃ dB scale
//! │▐ ███ -20│
//! │▐ ███ -40│
//! │▐ ▓▓▓ -60│
//! ╰─ 🔊 80% ╯  mute state and volume
//! ```
//!
//! The name and the mute readout ride on the strip's own border, top and
//! bottom, the way OBS frames them — on a terminal that is two rows the
//! meter would otherwise not get.
//!
//! **Reading the meter.** The bar is colored by three zones, using OBS's own
//! default thresholds. Green (below -20 dBFS) is where background music and
//! alert sounds belong. Yellow (-20 to -9 dBFS) is where speech should sit.
//! Red (above -9 dBFS) means the signal is close to *clipping* — the
//! distortion you hear when audio is too loud for the gear reproducing it.
//! "dBFS" is decibels relative to full scale, so 0 is the loudest a digital
//! signal can be and every value below it is negative.
//!
//! The bar itself is a peak program meter: it jumps up instantly and falls
//! off gradually, and a brighter marker rides above it holding the loudest
//! value from the last few seconds. Both come from
//! [`MeterReading`](crate::tui::model::MeterReading).
//!
//! When the rich UI is on, each cell of the meter is drawn as a half-block
//! (`▀`) with one color in its foreground and another in its background,
//! which doubles the vertical resolution a short terminal pane can show.

use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    obs::state::AudioState,
    tui::{
        anim,
        model::{FocusPanel, MeterReading, TuiModel},
        theme::Theme,
        widgets::chrome,
    },
};

/// Quietest level the meter shows; below this the input reads as silent.
/// OBS's meter bottoms out at the same -60 dBFS. Kept as both a positive
/// depth (for counting whole-decibel scale ticks) and the signed value the
/// meter maths uses, so the two can never drift apart.
const FLOOR_DEPTH_DB: u32 = 60;
const FLOOR_DB: f32 = -(FLOOR_DEPTH_DB as f32);
/// Zone boundaries in dBFS, matching OBS's default "error" and "warning"
/// meter levels.
const RED_DB: f32 = -9.0;
const YELLOW_DB: f32 = -20.0;

/// Outer size of one channel strip, its own border included, and the blank
/// column left between neighbouring strips.
///
/// Strips stretch to share out whatever the pane has spare, between the
/// narrowest that still fits a fader, a meter and a dB scale, and a cap that
/// stops two inputs from ballooning into half the screen each.
const STRIP_MIN_WIDTH: u16 = 11;
const STRIP_MAX_WIDTH: u16 = 16;
const STRIP_GAP: u16 = 1;
/// Columns reserved on the right of a strip for the dB scale (`-20`), plus
/// the blank column before it. Dropped entirely on strips too narrow to
/// afford both a scale and a meter worth looking at.
const SCALE_WIDTH: usize = 3;

/// Convert a linear magnitude (0-1, what OBS sends) into dBFS, floored at
/// [`FLOOR_DB`]. Silence is `log10(0)`, i.e. negative infinity, so anything
/// below a hair above zero is snapped to the floor instead.
fn linear_to_db(level: f32) -> f32 {
    if level < 1e-7 {
        FLOOR_DB
    } else {
        (20.0 * level.log10()).clamp(FLOOR_DB, 0.0)
    }
}

/// Zone color for a point on the meter at `db`.
fn zone_color(db: f32, theme: Theme) -> Color {
    if db >= RED_DB {
        theme.danger
    } else if db >= YELLOW_DB {
        theme.warning
    } else {
        theme.success
    }
}

/// The unlit version of a zone color — the meter's own background, so an
/// idle strip still shows where its zones are, the way OBS's does.
///
/// Blending needs truecolor on both sides. The `mono` theme uses named
/// terminal colors (and `Color::Reset` for its background), which have no
/// channels to mix, so those fall back to the plain border color.
fn unlit_color(zone: Color, theme: Theme) -> Color {
    match (zone, theme.bg) {
        (Color::Rgb(..), Color::Rgb(..)) => anim::blend(zone, theme.bg, 0.72),
        _ => theme.border,
    }
}

/// How many channel strips fit across `width` columns of pane interior, at
/// their narrowest.
pub fn visible_strips(width: u16) -> usize {
    if width < STRIP_MIN_WIDTH {
        return 0;
    }
    usize::from((width + STRIP_GAP) / (STRIP_MIN_WIDTH + STRIP_GAP)).max(1)
}

/// Index of the leftmost strip drawn, given where the cursor is.
///
/// Like the list panels, the audio matrix keeps no scroll state of its own:
/// the window is recomputed from the cursor every frame, which is what lets
/// mouse hit-testing reproduce the last frame's layout exactly.
fn first_visible_strip(count: usize, cursor: usize, visible: usize) -> usize {
    if visible == 0 || count <= visible {
        return 0;
    }
    cursor
        .saturating_sub(visible - 1)
        .min(count.saturating_sub(visible))
}

/// Where each visible channel strip lands inside `inner` (the pane's area
/// with its border already removed), as `(input index, strip rect)`.
///
/// The strips share the pane's width evenly and the whole row is centered,
/// so a matrix with one or two inputs does not sit lopsided against the left
/// border once the per-strip cap kicks in.
///
/// Shared with [`crate::tui::mouse`] so a click resolves against the same
/// geometry the frame was drawn with.
pub fn strip_layout(inner: Rect, count: usize, cursor: usize) -> Vec<(usize, Rect)> {
    let visible = visible_strips(inner.width);
    if visible == 0 || inner.height == 0 || count == 0 {
        return Vec::new();
    }
    let shown = visible.min(count);
    let gaps = (shown as u16 - 1) * STRIP_GAP;
    let width = (inner.width.saturating_sub(gaps) / shown as u16).min(STRIP_MAX_WIDTH);
    let used = width * shown as u16 + gaps;
    let left = inner.x + inner.width.saturating_sub(used) / 2;

    let first = first_visible_strip(count, cursor, visible);
    (first..count)
        .take(shown)
        .enumerate()
        .map(|(slot, index)| {
            let offset = slot as u16 * (width + STRIP_GAP);
            let rect = Rect {
                x: left.saturating_add(offset),
                y: inner.y,
                width,
                height: inner.height,
            };
            (index, rect)
        })
        .collect()
}

/// Input index under `pos`, or `None` for the gap between strips, the pane
/// border, or empty space past the last strip.
pub fn strip_index_at(inner: Rect, count: usize, cursor: usize, pos: Position) -> Option<usize> {
    strip_layout(inner, count, cursor)
        .into_iter()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(index, _)| index)
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let focused = model.focus == FocusPanel::Audio;
    let inputs = model.audio_inputs();

    let block = chrome::panel(
        model.symbol("🎚", "A"),
        "Audio Matrix",
        model.symbol("[a]  ↑↓ gain  m mute", "[a]  ^v gain  m mute"),
        inputs.len(),
        focused,
        model,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let strips = strip_layout(inner, inputs.len(), model.panel_cursor(FocusPanel::Audio));
    if strips.is_empty() {
        // Either there is nothing to show or the pane is too narrow for even
        // one strip; say so rather than leaving an unexplained empty box.
        let note = if inputs.is_empty() {
            "no audio inputs"
        } else {
            "too narrow"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                note,
                Style::default().fg(model.theme.muted),
            ))),
            inner,
        );
        return;
    }

    // One plan for the whole matrix: every strip devotes the same rows to
    // the same things, so the meters start and end level with each other and
    // their dB scales can be read straight across.
    let plan = RowPlan::for_height(
        usize::from(inner.height.saturating_sub(2)),
        inputs.iter().any(|input| strip_tag(input).is_some()),
    );

    for (index, rect) in strips {
        render_strip(
            f,
            rect,
            &inputs[index],
            plan,
            index == model.panel_cursor(FocusPanel::Audio),
            model,
        );
    }
}

/// How a strip's interior height is divided up.
///
/// The name and the mute/volume readout live on the strip's own border, top
/// and bottom, so every interior row is available to the meter and its two
/// optional companions. Those go out by priority — the gain readout first,
/// then the alias/shortcut tag — and only while the meter would still keep a
/// usable number of rows. Whatever is left is the meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowPlan {
    tag: bool,
    gain: bool,
    meter: usize,
}

impl RowPlan {
    fn for_height(height: usize, any_tag: bool) -> Self {
        let gain = height >= 5;
        let mut spare = height - usize::from(gain);
        let tag = any_tag && spare >= 6;
        spare -= usize::from(tag);
        Self {
            tag,
            gain,
            meter: spare,
        }
    }
}

fn render_strip(
    f: &mut Frame,
    rect: Rect,
    input: &AudioState,
    plan: RowPlan,
    selected: bool,
    model: &TuiModel,
) {
    let theme = model.theme;
    let focused = model.focus == FocusPanel::Audio;

    let border_color = match (selected, focused) {
        (true, true) if model.advanced_ui => {
            anim::blend(theme.border_focus, theme.accent_alt, 0.25)
        }
        (true, _) => theme.border_focus,
        (false, _) => theme.border,
    };
    let mut name_style = Style::default().fg(if selected { theme.accent } else { theme.fg });
    if selected {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    // Ratatui clips an over-long title from the left, which would leave a
    // name reading as "…r Capture"; trim it to what the border can hold
    // (two corners and a space either side) before handing it over.
    let name = truncate_to_cells(&input.name, usize::from(rect.width).saturating_sub(4));

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Thick
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(border_color))
        .title_top(Line::from(Span::styled(format!(" {name} "), name_style)).centered())
        .title_bottom(footer_line(input, model).centered());
    if !model.advanced_ui {
        block = block.border_set(chrome::ASCII_BORDER);
    }

    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines = strip_lines(
        input,
        model.meter(&input.name),
        usize::from(inner.width),
        plan,
        model,
    );
    f.render_widget(Paragraph::new(lines), inner);
}

/// Build the strip's contents for an interior `width` cells wide, laid out
/// according to the matrix-wide [`RowPlan`].
fn strip_lines(
    input: &AudioState,
    meter: Option<MeterReading>,
    width: usize,
    plan: RowPlan,
    model: &TuiModel,
) -> Vec<Line<'static>> {
    let theme = model.theme;
    let muted = input.muted.unwrap_or(false);
    let meter_rows = plan.meter;
    let mut lines: Vec<Line<'static>> = Vec::new();

    if plan.tag {
        // An input with no alias or shortcut still spends the row, so the
        // strips beside it keep their meters at the same height.
        let tag = strip_tag(input).unwrap_or_default();
        lines.push(Line::from(Span::styled(
            pad_center(&tag, width),
            Style::default().fg(theme.muted),
        )));
    }

    if plan.gain {
        lines.push(Line::from(Span::styled(
            pad_center(&gain_text(input), width),
            Style::default().fg(if muted { theme.muted } else { theme.info }),
        )));
    }

    let reading = meter.unwrap_or_default();
    let column = MeterColumn {
        bar_db: linear_to_db(reading.peak_program),
        hold_db: linear_to_db(reading.peak_hold),
        volume_percent: input.volume_percent,
        muted,
    };
    for (row, label) in scale_labels(meter_rows).iter().enumerate() {
        lines.push(meter_line(
            row,
            meter_rows,
            width,
            column,
            label.as_deref(),
            model,
        ));
    }

    lines
}

/// The alias/shortcut row, e.g. `mic [1]`, or `None` when the input has
/// neither. These are obsctl's own naming shortcuts, not anything OBS knows
/// about, so they are the first thing dropped when a strip is short.
fn strip_tag(input: &AudioState) -> Option<String> {
    match (input.alias.as_deref(), input.shortcut.as_deref()) {
        (Some(alias), Some(shortcut)) => Some(format!("{alias} [{shortcut}]")),
        (Some(alias), None) => Some(alias.to_string()),
        (None, Some(shortcut)) => Some(format!("[{shortcut}]")),
        (None, None) => None,
    }
}

/// The fader's gain, preferring OBS's own dB value and falling back to the
/// percentage when the snapshot has not carried one yet.
fn gain_text(input: &AudioState) -> String {
    match (input.volume_db, input.volume_percent) {
        // OBS reports -inf dB for a fader pulled all the way down.
        (Some(db), _) if db.is_finite() && db > -100.0 => format!("{db:.1} dB"),
        (Some(_), _) => "-inf dB".to_string(),
        (None, Some(percent)) => format!("{percent}%"),
        (None, None) => String::new(),
    }
}

/// Mute state and volume, drawn along the strip's bottom border where OBS
/// puts its own speaker toggle.
fn footer_line(input: &AudioState, model: &TuiModel) -> Line<'static> {
    let theme = model.theme;
    let (icon, color) = match input.muted {
        Some(true) => (model.symbol("🔇", "M"), theme.danger),
        Some(false) => (model.symbol("🔊", "A"), theme.success),
        // OBS has not told us whether this input is muted yet.
        None => (model.symbol("〰", "-"), theme.muted),
    };
    let value = match (input.muted, input.volume_percent) {
        (Some(true), _) => "mute".to_string(),
        (_, Some(percent)) => format!("{percent}%"),
        (_, None) => String::new(),
    };
    let text = if value.is_empty() {
        format!(" {icon} ")
    } else {
        format!(" {icon} {value} ")
    };
    Line::from(Span::styled(text, Style::default().fg(color)))
}

/// Everything the fader and meter of one strip need in order to color
/// themselves, hoisted out of the per-row arguments.
#[derive(Debug, Clone, Copy)]
struct MeterColumn {
    /// Peak program level in dBFS — how far up the bar reaches.
    bar_db: f32,
    /// Peak hold marker in dBFS.
    hold_db: f32,
    /// Fader position, when OBS has reported a volume for this input.
    volume_percent: Option<u8>,
    muted: bool,
}

/// How a strip's interior splits sideways.
struct MeterColumns {
    meter: usize,
    /// Zero on strips too narrow to afford both a scale and a meter worth
    /// looking at, in which case the meter takes those columns instead.
    scale: usize,
}

/// fader (1) + gap (1) + meter + gap (1) + scale (3)
fn meter_columns(width: usize) -> MeterColumns {
    let scale = if width >= SCALE_WIDTH + 5 {
        SCALE_WIDTH
    } else {
        0
    };
    let reserved = 2 + if scale > 0 { scale + 1 } else { 0 };
    MeterColumns {
        meter: width.saturating_sub(reserved),
        scale,
    }
}

/// dB range one sub-row of the meter covers, top edge first.
fn sub_bounds(sub: usize, subs: usize) -> (f32, f32) {
    (
        FLOOR_DB * (sub as f32 / subs.max(1) as f32),
        FLOOR_DB * ((sub + 1) as f32 / subs.max(1) as f32),
    )
}

/// What one sub-row of the meter is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeterMark {
    /// The peak-hold marker, parked at the loudest recent level.
    Peak,
    /// Inside the bar.
    Lit,
    /// Above the bar — the meter's own dimmed background.
    Unlit,
}

fn meter_mark(sub: usize, subs: usize, column: MeterColumn) -> MeterMark {
    let (top, bottom) = sub_bounds(sub, subs);
    if column.hold_db > FLOOR_DB && column.hold_db <= top && column.hold_db > bottom {
        MeterMark::Peak
    } else if column.bar_db > bottom {
        MeterMark::Lit
    } else {
        MeterMark::Unlit
    }
}

fn meter_color(mark: MeterMark, sub: usize, subs: usize, muted: bool, theme: Theme) -> Color {
    let (top, bottom) = sub_bounds(sub, subs);
    let zone = zone_color((top + bottom) / 2.0, theme);
    match mark {
        MeterMark::Unlit => unlit_color(zone, theme),
        _ if muted => theme.muted,
        MeterMark::Peak => theme.fg,
        MeterMark::Lit => zone,
    }
}

/// One cell of a vertical bar with the rich UI on: a half-block showing two
/// independent sub-rows, `top` in the foreground and `bottom` behind it.
/// This is what doubles the vertical resolution a short pane can express.
fn stacked_cell(top: Color, bottom: Color) -> Span<'static> {
    Span::styled("▀", Style::default().fg(top).bg(bottom))
}

/// One row of the fader / meter / scale block.
///
/// `row` counts down from the top, where 0 dBFS lives; `rows` is how many
/// rows the whole -60..0 dBFS range is spread across.
fn meter_line(
    row: usize,
    rows: usize,
    width: usize,
    column: MeterColumn,
    label: Option<&str>,
    model: &TuiModel,
) -> Line<'static> {
    let columns = meter_columns(width);
    // Each cell covers two sub-rows when half-blocks are available.
    let subs = if model.advanced_ui { rows * 2 } else { rows };
    let (top_sub, bottom_sub) = if model.advanced_ui {
        (row * 2, row * 2 + 1)
    } else {
        (row, row)
    };

    let mut spans = Vec::with_capacity(4 + columns.meter);
    spans.push(fader_span(top_sub, bottom_sub, subs, column, model));
    spans.push(Span::raw(" "));
    spans.extend(meter_spans(
        columns.meter,
        top_sub,
        bottom_sub,
        subs,
        column,
        model,
    ));
    if columns.scale > 0 {
        spans.push(Span::raw(" "));
        spans.push(scale_span(label, columns.scale, model.theme));
    }
    Line::from(spans)
}

/// The vertical volume slider down the left of the strip, with its knob
/// parked at the input's current volume.
fn fader_span(
    top_sub: usize,
    bottom_sub: usize,
    subs: usize,
    column: MeterColumn,
    model: &TuiModel,
) -> Span<'static> {
    let theme = model.theme;
    let knob = column.volume_percent.map(|percent| {
        let travel = subs.saturating_sub(1) as f32;
        ((1.0 - f32::from(percent) / 100.0) * travel).round() as usize
    });
    let color = |sub| {
        if knob == Some(sub) {
            theme.fg
        } else {
            unlit_color(theme.info, theme)
        }
    };
    if model.advanced_ui {
        stacked_cell(color(top_sub), color(bottom_sub))
    } else if knob == Some(top_sub) {
        Span::styled("#", Style::default().fg(theme.fg))
    } else {
        Span::styled("|", Style::default().fg(theme.info))
    }
}

/// The meter itself. Every cell of a row shows the same thing, so the row is
/// one span repeated across the meter's width.
fn meter_spans(
    cells: usize,
    top_sub: usize,
    bottom_sub: usize,
    subs: usize,
    column: MeterColumn,
    model: &TuiModel,
) -> Vec<Span<'static>> {
    let theme = model.theme;
    let color = |sub| {
        meter_color(
            meter_mark(sub, subs, column),
            sub,
            subs,
            column.muted,
            theme,
        )
    };
    let cell = if model.advanced_ui {
        stacked_cell(color(top_sub), color(bottom_sub))
    } else {
        let mark = meter_mark(top_sub, subs, column);
        let glyph = match mark {
            MeterMark::Peak => "=",
            MeterMark::Lit => "#",
            MeterMark::Unlit => ".",
        };
        Span::styled(glyph, Style::default().fg(color(top_sub)))
    };
    vec![cell; cells]
}

/// This row's dB tick, right-aligned so the scale reads as a ruled column.
fn scale_span(label: Option<&str>, width: usize, theme: Theme) -> Span<'static> {
    let text = label.unwrap_or("");
    Span::styled(format!("{text:>width$}"), Style::default().fg(theme.muted))
}

/// dB tick label for each meter row, or `None` for rows that get none.
///
/// The step widens on short panes so the few rows available do not all end
/// up labelled — six rows get `0 / -20 / -40 / -60`, a tall pane gets every
/// 6 dB, the way OBS's own scale is ruled.
fn scale_labels(rows: usize) -> Vec<Option<String>> {
    let mut labels = vec![None; rows];
    if rows == 0 {
        return labels;
    }
    // Whole decibels, counted rather than accumulated: stepping a float down
    // to the floor would decide how many ticks there are by rounding error.
    let step: u32 = match rows {
        0..=1 => FLOOR_DEPTH_DB,
        2..=3 => 30,
        4..=6 => 20,
        7..=10 => 12,
        _ => 6,
    };
    for tick in 0..=(FLOOR_DEPTH_DB / step) {
        let depth = tick * step;
        let row = ((depth as f32 / FLOOR_DEPTH_DB as f32) * rows as f32).floor() as usize;
        let row = row.min(rows - 1);
        if labels[row].is_none() {
            labels[row] = Some(if depth == 0 {
                "0".to_string()
            } else {
                format!("-{depth}")
            });
        }
    }
    labels
}

/// Longest prefix of `text` that fits in `cells` terminal cells.
///
/// Counting characters is not the same as counting cells: an emoji or a CJK
/// ideograph occupies two. Truncating by character would let a name overflow
/// its strip, and ratatui would then clip the row's last cell.
fn truncate_to_cells(text: &str, cells: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.width().unwrap_or(0);
        if used + width > cells {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

/// Center `text` in exactly `width` cells, truncating when it does not fit.
fn pad_center(text: &str, width: usize) -> String {
    let trimmed = truncate_to_cells(text, width);
    let len = trimmed.width();
    let left = width.saturating_sub(len) / 2;
    let right = width.saturating_sub(left + len);
    let mut padded = " ".repeat(left);
    padded.push_str(&trimmed);
    padded.push_str(&" ".repeat(right));
    padded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: u16, height: u16) -> Rect {
        Rect::new(0, 0, width, height)
    }

    #[test]
    fn silence_reads_as_the_meter_floor() {
        assert_eq!(linear_to_db(0.0), FLOOR_DB);
        assert_eq!(linear_to_db(1e-9), FLOOR_DB);
    }

    #[test]
    fn full_scale_reads_as_zero_db() {
        assert!((linear_to_db(1.0) - 0.0).abs() < 1e-4);
        // OBS can briefly report above full scale; the meter tops out at 0.
        assert_eq!(linear_to_db(2.0), 0.0);
    }

    #[test]
    fn zones_follow_obs_warning_and_error_levels() {
        let theme = Theme::default_theme();
        assert_eq!(zone_color(-3.0, theme), theme.danger);
        assert_eq!(zone_color(RED_DB, theme), theme.danger);
        assert_eq!(zone_color(-15.0, theme), theme.warning);
        assert_eq!(zone_color(YELLOW_DB, theme), theme.warning);
        assert_eq!(zone_color(-45.0, theme), theme.success);
    }

    #[test]
    fn strips_fit_side_by_side_with_a_gap_between_them() {
        // 11-wide strips, one blank column apart: 11 + 1 + 11 + 1 + 11 = 35.
        assert_eq!(visible_strips(35), 3);
        assert_eq!(visible_strips(34), 2);
        assert_eq!(visible_strips(23), 2);
        assert_eq!(visible_strips(22), 1);
        assert_eq!(visible_strips(11), 1);
        assert_eq!(visible_strips(10), 0);
        assert_eq!(visible_strips(0), 0);
    }

    #[test]
    fn strip_layout_places_each_strip_after_the_previous_one() {
        let strips = strip_layout(rect(35, 10), 3, 0);
        let xs: Vec<u16> = strips.iter().map(|(_, r)| r.x).collect();
        assert_eq!(xs, vec![0, 12, 24]);
        assert!(strips.iter().all(|(_, r)| r.width == STRIP_MIN_WIDTH));
    }

    #[test]
    fn strips_share_out_the_spare_width_and_the_row_stays_centered() {
        // 44 columns, three strips: 14 each plus two gaps fills it exactly.
        let strips = strip_layout(rect(44, 10), 3, 0);
        assert!(strips.iter().all(|(_, r)| r.width == 14));
        assert_eq!(
            strips.iter().map(|(_, r)| r.x).collect::<Vec<_>>(),
            vec![0, 15, 30]
        );

        // Two inputs in the same pane hit the per-strip cap, so the leftover
        // is split between the two edges instead of trailing off the right.
        let pair = strip_layout(rect(44, 10), 2, 0);
        assert!(pair.iter().all(|(_, r)| r.width == STRIP_MAX_WIDTH));
        assert_eq!(pair[0].1.x, 5);
        assert_eq!(pair[1].1.x, 22);
    }

    #[test]
    fn the_window_scrolls_sideways_to_keep_the_cursor_visible() {
        // Six inputs, three visible: the cursor pins to the right edge.
        assert_eq!(first_visible_strip(6, 0, 3), 0);
        assert_eq!(first_visible_strip(6, 2, 3), 0);
        assert_eq!(first_visible_strip(6, 3, 3), 1);
        assert_eq!(first_visible_strip(6, 5, 3), 3);
        // Nothing scrolls when everything fits.
        assert_eq!(first_visible_strip(2, 1, 3), 0);
        assert_eq!(first_visible_strip(0, 0, 0), 0);
    }

    #[test]
    fn strip_layout_shows_the_cursor_even_when_it_is_off_the_right_edge() {
        let strips = strip_layout(rect(35, 10), 6, 5);
        let indices: Vec<usize> = strips.iter().map(|(index, _)| *index).collect();
        assert_eq!(indices, vec![3, 4, 5]);
    }

    #[test]
    fn strip_index_at_ignores_the_gaps_between_strips() {
        let inner = rect(35, 10);
        assert_eq!(strip_index_at(inner, 3, 0, Position::new(0, 0)), Some(0));
        assert_eq!(strip_index_at(inner, 3, 0, Position::new(10, 9)), Some(0));
        // Column 11 is the blank gap column.
        assert_eq!(strip_index_at(inner, 3, 0, Position::new(11, 0)), None);
        assert_eq!(strip_index_at(inner, 3, 0, Position::new(12, 0)), Some(1));
        assert_eq!(strip_index_at(inner, 3, 0, Position::new(24, 0)), Some(2));
        assert_eq!(strip_index_at(inner, 3, 0, Position::new(34, 0)), Some(2));
        // Empty space past the last strip, with only two inputs to draw.
        assert_eq!(strip_index_at(inner, 2, 0, Position::new(34, 0)), None);
    }

    #[test]
    fn every_strip_gets_the_same_row_plan_however_short_the_pane_is() {
        // Tall enough for the lot: name, tag, gain, meter, footer.
        let tall = RowPlan::for_height(12, true);
        assert!(tall.tag && tall.gain);
        assert_eq!(tall.meter, 10);

        // With no alias or shortcut anywhere, that row goes to the meter.
        assert_eq!(RowPlan::for_height(12, false).meter, 11);

        // As the pane shrinks the extras drop away in priority order, and
        // the meter is the last thing standing.
        assert!(RowPlan::for_height(7, true).tag);
        assert!(!RowPlan::for_height(6, true).tag);
        assert!(RowPlan::for_height(5, true).gain);
        assert!(!RowPlan::for_height(4, true).gain);
        assert_eq!(
            RowPlan::for_height(4, true),
            RowPlan {
                tag: false,
                gain: false,
                meter: 4,
            }
        );
        assert_eq!(RowPlan::for_height(0, true).meter, 0);
    }

    #[test]
    fn scale_labels_widen_their_step_on_short_meters() {
        let six = scale_labels(6);
        let printed: Vec<&str> = six.iter().filter_map(|l| l.as_deref()).collect();
        assert_eq!(printed, vec!["0", "-20", "-40", "-60"]);

        let twelve = scale_labels(12);
        assert_eq!(twelve.iter().filter(|l| l.is_some()).count(), 11);
        assert_eq!(twelve[0].as_deref(), Some("0"));

        assert!(scale_labels(0).is_empty());
    }

    #[test]
    fn pad_center_always_returns_exactly_the_requested_width() {
        assert_eq!(pad_center("Mic", 9), "   Mic   ");
        // An odd remainder favours the right, so the text never drifts left
        // of centre as a strip widens.
        assert_eq!(pad_center("Mic", 4), "Mic ");
        assert_eq!(pad_center("Desktop Audio", 9).chars().count(), 9);
        assert_eq!(pad_center("Mic", 0), "");
    }

    #[test]
    fn gain_prefers_obs_decibels_and_falls_back_to_percent() {
        let db = AudioState {
            volume_db: Some(-3.25),
            volume_percent: Some(70),
            ..Default::default()
        };
        assert_eq!(gain_text(&db), "-3.2 dB");

        let silent = AudioState {
            volume_db: Some(f64::NEG_INFINITY),
            ..Default::default()
        };
        assert_eq!(gain_text(&silent), "-inf dB");

        let percent_only = AudioState {
            volume_percent: Some(40),
            ..Default::default()
        };
        assert_eq!(gain_text(&percent_only), "40%");

        assert_eq!(gain_text(&AudioState::default()), "");
    }

    #[test]
    fn the_tag_row_combines_alias_and_shortcut() {
        let both = AudioState {
            alias: Some("mic".into()),
            shortcut: Some("1".into()),
            ..Default::default()
        };
        assert_eq!(strip_tag(&both).as_deref(), Some("mic [1]"));
        assert_eq!(strip_tag(&AudioState::default()), None);
    }
}
