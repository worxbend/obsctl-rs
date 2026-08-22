use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::tui::{anim, model::TuiModel};

pub const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// A plain bordered box in the theme's chrome, for the parts of the UI that
/// are not selectable list panels — the header, the status pane, the live bar,
/// the settings view, the connection notice.
///
/// Every one of them built the same `Block` by hand and then remembered to
/// swap in the ASCII border set when the model is in no-icon mode. Forgetting
/// that last step is invisible in a normal terminal and only shows up as
/// mojibake for the users who asked for ASCII output, so it happens here
/// instead of at seven call sites.
pub fn bordered<'a>(
    model: &TuiModel,
    border_type: BorderType,
    border_color: Color,
    title: impl Into<Line<'a>>,
) -> Block<'a> {
    ascii_aware(
        Block::default()
            .borders(Borders::ALL)
            .border_type(border_type)
            .border_style(Style::default().fg(border_color))
            .title(title),
        model,
    )
}

/// Pick between a box-drawing / chart / typography glyph and its plain-ASCII
/// stand-in, on `advanced_ui` alone.
///
/// The model has two separate switches. `show_icons` is about emoji and
/// pictograms — 🎬, ⚡, ✔ — which some terminals render at the wrong width or
/// not at all. `advanced_ui` is about whether the terminal can do Unicode
/// box drawing and shading at all. [`TuiModel::symbol`] is the twin of this
/// function for the first switch (it is gated on `rich_ui()`, meaning both
/// must be on); this one is for the second.
///
/// Widgets used to write the second decision out inline, thirty-five times,
/// and one of them reached for the wrong switch — so with icons off but
/// Unicode on, the status pane drew its big letters out of `#` while the
/// settings swatches and the stats bars beside it stayed `█`. Naming the
/// decision is what stops the two from being answered differently.
pub fn glyph(model: &TuiModel, advanced: &'static str, plain: &'static str) -> &'static str {
    if model.advanced_ui { advanced } else { plain }
}

/// [`glyph`] for the call sites that build a string a character at a time,
/// such as the fill and empty cells of a bar.
pub fn glyph_char(model: &TuiModel, advanced: char, plain: char) -> char {
    if model.advanced_ui { advanced } else { plain }
}

/// The vertical rule drawn between two readouts sharing one row.
///
/// The live bar and the status pane each had their own copy, identical down
/// to the two spaces either side; one file changing its gutter would have
/// silently unaligned the two rows the user sees stacked on top of each
/// other.
pub fn separator(model: &TuiModel) -> Span<'static> {
    Span::styled(
        glyph(model, "  │  ", "  |  "),
        Style::default().fg(model.theme.border),
    )
}

/// Rewrite the typographic punctuation an ASCII-only terminal cannot show —
/// the ellipsis `…` and the em dash `—` — into plain equivalents, unless the
/// model is drawing the advanced UI.
///
/// Five messages used to be stored twice, once spelled with the typographic
/// character and once with the ASCII one, sitting in the two arms of an
/// `if`. That is two chances to fix a wording and only remember one of them,
/// and it had already produced five slightly different spellings of the same
/// idea. Writing the sentence once in its typographic form and passing it
/// through here keeps the pair in step — which is what `connection` was
/// already doing by hand for its em dash.
pub fn typographic(model: &TuiModel, text: &str) -> String {
    if model.advanced_ui {
        text.to_string()
    } else {
        text.replace('…', "...").replace('—', "-")
    }
}

/// Swap a block's box-drawing border for the ASCII one when the model is in
/// no-icon mode. Both [`bordered`] and [`panel`] apply this already; call it
/// directly only for a block built some other way.
pub fn ascii_aware<'a>(block: Block<'a>, model: &TuiModel) -> Block<'a> {
    if model.advanced_ui {
        block
    } else {
        block.border_set(ASCII_BORDER)
    }
}

/// A panel or view heading: an animated per-character gradient when the model
/// is drawing the advanced UI, and a plain bold line in `from` when it is not.
///
/// Six headings — the panel titles, the live bar, the settings view, the
/// header and the connection notice — each wrote this `if` out themselves,
/// with only the two colours differing. Writing it once means a change to how
/// a heading looks lands on all of them instead of on whichever five the
/// author remembered.
pub fn heading(model: &TuiModel, text: impl Into<String>, from: Color, to: Color) -> Line<'static> {
    heading_inner(model, text.into(), from, to, true)
}

/// [`heading`] without the bold on the non-animated fallback.
///
/// This exists for exactly one caller — the settings preview heading — which
/// has always drawn its fallback at the lighter weight. Whether that was
/// intended or an oversight is not recorded anywhere, so it is preserved
/// rather than quietly normalised; see the comment at that call site.
pub fn heading_unbolded(
    model: &TuiModel,
    text: impl Into<String>,
    from: Color,
    to: Color,
) -> Line<'static> {
    heading_inner(model, text.into(), from, to, false)
}

fn heading_inner(
    model: &TuiModel,
    text: String,
    from: Color,
    to: Color,
    bold: bool,
) -> Line<'static> {
    if model.advanced_ui {
        anim::gradient_line(&text, from, to, model.anim.frame, true)
    } else {
        let mut style = Style::default().fg(from);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        // A span rather than a styled `Line`, so callers that take the line
        // apart — `panel` appends its count badge to these spans — carry the
        // styling with them. Both render identically.
        Line::from(Span::styled(text, style))
    }
}

/// How far a focused border is tinted toward the theme's secondary accent, so
/// the focused panel reads as warmer rather than merely brighter.
const FOCUS_ACCENT_MIX: f32 = 0.25;

/// The border weight and colour that say "this is the panel the keyboard is
/// talking to": a thick frame tinted toward the accent, against the rounded
/// plain frame everything else gets.
///
/// The panel chrome and the audio strips each decided this themselves, which
/// is how the two ended up one edit away from disagreeing about what focus
/// looks like.
pub fn focus_frame(model: &TuiModel, focused: bool) -> (BorderType, Color) {
    let border_type = if focused {
        BorderType::Thick
    } else {
        BorderType::Rounded
    };
    let color = if focused && model.advanced_ui {
        anim::blend(
            model.theme.border_focus,
            model.theme.accent_alt,
            FOCUS_ACCENT_MIX,
        )
    } else if focused {
        model.theme.border_focus
    } else {
        model.theme.border
    };
    (border_type, color)
}

/// A border colour that breathes on the shared pulse: `base` tinted up to
/// `amount` of the way toward `toward`, and plain `base` when the model is not
/// drawing the advanced UI.
///
/// The live bar, the status pane and the connection notice all animate their
/// frame this way; only the two colours and how far the tint travels differ.
pub fn breathing_border(model: &TuiModel, base: Color, toward: Color, amount: f32) -> Color {
    if model.advanced_ui {
        anim::blend(
            base,
            toward,
            model.anim.pulse(anim::PULSE_PERIOD_TICKS) * amount,
        )
    } else {
        base
    }
}

/// A fixed-width filled/empty bar. `None` (nothing measured yet) draws as all
/// empty, so the row keeps its width instead of collapsing.
///
/// The live bar's ASCII meter and the stats pane's frame-budget bar built the
/// same string with the same two glyph pairs; the ratio and the width are the
/// only things that legitimately differ.
pub fn ratio_bar(model: &TuiModel, ratio: Option<f64>, width: usize) -> String {
    let full = glyph_char(model, '█', '#');
    let empty = glyph_char(model, '░', '-');
    let Some(ratio) = ratio else {
        return std::iter::repeat_n(empty, width).collect();
    };
    let filled = (ratio.clamp(0.0, 1.0) * width as f64).round() as usize;
    std::iter::repeat_n(full, filled)
        .chain(std::iter::repeat_n(empty, width - filled))
        .collect()
}

/// Shared panel chrome: rounded inactive borders, a heavier focused frame,
/// animated gradient headings, a count badge, and a dim keyboard hint.
pub fn panel<'a>(
    icon: &'a str,
    label: &'a str,
    hint: &'a str,
    count: usize,
    focused: bool,
    model: &TuiModel,
) -> Block<'a> {
    let theme = model.theme;
    // Keep a full visual gutter after wide emoji glyphs. Some terminals render
    // emoji in two cells, making a single following space appear collapsed.
    let title_text = format!(" {icon}  {label} ");
    let mut title_spans = heading(model, title_text, theme.accent, theme.accent_alt).spans;
    title_spans.push(Span::styled(
        format!(" {count:02} "),
        Style::default()
            .fg(theme.highlight_fg)
            .bg(theme.highlight_bg)
            .add_modifier(Modifier::BOLD),
    ));
    if !hint.is_empty() {
        title_spans.push(Span::styled(
            format!("  {hint} "),
            Style::default().fg(theme.muted),
        ));
    }

    let (border_type, border_color) = focus_frame(model, focused);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_spans));
    ascii_aware(block, model)
}

/// Draw `block` into `area` and hand back the interior left to fill, or
/// `None` when that interior has collapsed to nothing.
///
/// Drawing a panel is always the same three steps: measure the interior of
/// the border, paint the border, then stop if there is no interior left. That
/// last case happens on genuinely tiny terminals, where a pane can be squeezed
/// down to fewer columns than its own border needs. Only four of the eleven
/// panels wrote the check out; the rest were relying on Ratatui quietly
/// clipping whatever they drew, which works but is indistinguishable from
/// having forgotten. Going through here makes the check part of drawing a
/// frame rather than something each panel remembers separately.
///
/// Callers read as `let Some(inner) = chrome::frame(f, area, block) else { return; };`
pub fn frame(f: &mut Frame, area: Rect, block: Block) -> Option<Rect> {
    let inner = block.inner(area);
    f.render_widget(block, area);
    (inner.width > 0 && inner.height > 0).then_some(inner)
}

/// The muted one-liner a panel shows in place of content it does not have:
/// no audio inputs, no metrics from OBS yet, nothing to complete.
///
/// Five panels each built this line themselves, and had already drifted apart
/// on styling. The wording — and the indentation baked into it, which lines
/// the text up with whatever else that panel draws — stays with the caller,
/// because that part legitimately differs; the colour is decided here so an
/// empty pane reads the same wherever the user meets one.
pub fn placeholder(model: &TuiModel, text: impl Into<String>) -> Paragraph<'static> {
    Paragraph::new(placeholder_line(model, text))
}

/// [`placeholder`] as a bare [`Line`], for the panels that fold their empty
/// state into a paragraph they are already assembling line by line.
pub fn placeholder_line(model: &TuiModel, text: impl Into<String>) -> Line<'static> {
    Line::from(placeholder_span(model, text))
}

/// [`placeholder`] as a bare [`Span`], for the two spots where the empty
/// state is one phrase inside a row that also carries other things — the live
/// bar's throughput slot and its compact single-line fallback.
pub fn placeholder_span(model: &TuiModel, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(model.theme.muted))
}

pub fn status_dot(active: bool, fancy: bool) -> &'static str {
    match (active, fancy) {
        (true, true) => "●",
        (false, true) => "○",
        (true, false) => "*",
        (false, false) => "-",
    }
}

/// Elapsed stream/record time as `mm:ss`, widening to `hh:mm:ss` only once
/// the session passes an hour so short sessions stay narrow. `None` (OBS has
/// not reported a duration yet) renders as a placeholder of the same width.
///
/// Shared by the live bar and the top-right status pane so both spell a
/// running broadcast the same way.
pub fn format_duration(ms: Option<u64>) -> String {
    let Some(ms) = ms else {
        return "--:--".to_string();
    };
    let total_secs = ms / 1000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_handles_none() {
        assert_eq!(format_duration(None), "--:--");
    }

    #[test]
    fn format_duration_formats_minutes_and_seconds() {
        assert_eq!(format_duration(Some(65_000)), "01:05");
    }

    #[test]
    fn format_duration_formats_hours_when_long() {
        assert_eq!(format_duration(Some(3_661_000)), "01:01:01");
    }
}
