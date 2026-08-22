//! The telemetry strip under the header: what scene is live, how the encoder
//! is doing, and — on terminals too narrow for the top-right status pane —
//! inline IDLE/LIVE/REC badges.
//!
//! CPU and memory are drawn as `ratatui-braille-bar` meters rather than
//! sparklines: a sparkline answers "how has this moved", a meter answers "how
//! close to the limit is this right now", which is the question that matters
//! mid-broadcast. Each meter carries a peak marker so a spike that has already
//! passed stays visible.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{BorderType, Paragraph},
};
use ratatui_braille_bar::BrailleBar;
use rust_i18n::t;

use crate::tui::widgets::chrome;
use crate::tui::{anim, model::TuiModel, spinner, theme::Theme};

/// Cells per braille meter. Ten cells is enough to read a rough percentage
/// while leaving the labels and values room on a narrow strip.
const METER_WIDTH: usize = 10;

/// Memory has no natural ceiling the way CPU has 100%, so the meter scales to
/// the next multiple of this (in MB) above the session peak. Rounding to a
/// step keeps the scale from shifting under the bar on every sample.
const MEMORY_SCALE_STEP_MB: f64 = 256.0;

/// `include_badges` asks for the inline IDLE/LIVE/REC badges. The caller
/// passes `false` when [`crate::tui::widgets::status`] is on screen, so the
/// broadcast state is stated once rather than twice.
pub fn render(f: &mut Frame, area: Rect, model: &TuiModel, include_badges: bool) {
    let theme = model.theme;
    let pulse = model.anim.pulse(anim::PULSE_PERIOD_TICKS);
    let active = model.streaming() || model.recording();
    let border = if active {
        chrome::breathing_border(model, theme.border, theme.danger, 0.8)
    } else {
        chrome::breathing_border(model, theme.border, theme.info, 0.18)
    };
    let title_text = format!(
        "{}{} ",
        chrome::glyph(model, " ◉ ", " "),
        t!("tui.panels.live_bar.title")
    );
    let title = chrome::heading(model, title_text, theme.danger, theme.warning);
    let block = chrome::bordered(model, BorderType::Rounded, border, title);
    let Some(inner) = chrome::frame(f, area, block) else {
        return;
    };

    let mut first = Vec::new();
    if include_badges {
        first.extend(badges(model, pulse));
        first.push(chrome::separator(model));
    }
    first.push(Span::styled(
        format!(
            "{} {}",
            model.symbol("🎬", "SCN"),
            model
                .current_scene()
                .map(str::to_string)
                .unwrap_or_else(|| t!("tui.panels.live_bar.no_active_scene").into_owned())
        ),
        Style::default().fg(theme.fg),
    ));

    if inner.height < 2 {
        first.push(chrome::separator(model));
        first.push(compact_metrics(model, theme));
        f.render_widget(Paragraph::new(Line::from(first)), inner);
        return;
    }

    first.extend(throughput_spans(model, theme));
    f.render_widget(
        Paragraph::new(vec![Line::from(first), meters_line(model, theme)]),
        inner,
    );
}

/// Inline state badges for the narrow layout — one per active state, or a
/// single IDLE badge when neither the stream nor the recording is running.
fn badges(model: &TuiModel, pulse: f32) -> Vec<Span<'static>> {
    let theme = model.theme;
    let states = spinner::active_states(model.streaming(), model.recording());
    let mut spans = Vec::with_capacity(states.len() * 2);
    for (index, state) in states.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
        }
        let frame = spinner::frame(*state, model.rich_ui(), model.anim.frame);
        let detail = match state {
            spinner::BroadcastState::Idle => String::new(),
            spinner::BroadcastState::Live => chrome::format_duration(model.stream_duration_ms()),
            spinner::BroadcastState::Rec => chrome::format_duration(model.record_duration_ms()),
        };
        let text = if detail.is_empty() {
            format!(" {frame} {} ", state.label())
        } else {
            format!(" {frame} {} {detail} ", state.label())
        };
        let style = match state {
            spinner::BroadcastState::Idle => Style::default().fg(theme.muted),
            spinner::BroadcastState::Live => Style::default()
                .fg(anim::blend(theme.danger, theme.warning, pulse * 0.45))
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
            spinner::BroadcastState::Rec => Style::default()
                .fg(anim::blend(theme.warning, theme.danger, pulse * 0.45))
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        };
        spans.push(Span::styled(text, style));
    }
    spans
}

/// FPS and network throughput — the numbers that share the scene row.
fn throughput_spans(model: &TuiModel, theme: Theme) -> Vec<Span<'static>> {
    let Some(stats) = model.stats() else {
        return vec![
            chrome::separator(model),
            chrome::placeholder_span(
                model,
                chrome::typographic(model, &t!("tui.stats.waiting_for_metrics")),
            ),
        ];
    };
    let graph = if model.advanced_ui {
        anim::sparkline
    } else {
        anim::sparkline_ascii
    };
    let bitrate = bitrate_text(model);
    vec![
        chrome::separator(model),
        Span::styled(
            format!("FPS {:.1}", stats.active_fps),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        chrome::separator(model),
        Span::styled(
            format!(
                "NET {bitrate} {}",
                graph(model.bitrate_history.samples(), 8)
            ),
            Style::default()
                .fg(theme.accent_alt)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

/// The CPU and memory meter row.
fn meters_line(model: &TuiModel, theme: Theme) -> Line<'static> {
    let Some(stats) = model.stats() else {
        return chrome::placeholder_line(
            model,
            format!(
                "{} {}",
                model.symbol("⌁", "~"),
                chrome::typographic(model, &t!("tui.panels.live_bar.telemetry_idle"))
            ),
        );
    };

    let cpu_peak = model.cpu_history.peak(stats.cpu_usage_percent);
    let memory_peak = model.memory_history.peak(stats.memory_usage_mb);

    let mut spans = meter(
        model,
        "CPU",
        format!("{:>5.1}%", stats.cpu_usage_percent),
        stats.cpu_usage_percent,
        cpu_peak,
        100.0,
        theme.warning,
    );
    spans.push(chrome::separator(model));
    spans.extend(meter(
        model,
        "MEM",
        format!("{:>5.0}MB", stats.memory_usage_mb),
        stats.memory_usage_mb,
        memory_peak,
        memory_scale(memory_peak),
        theme.info,
    ));
    Line::from(spans)
}

/// One `LABEL value ▁bar▁` group.
fn meter(
    model: &TuiModel,
    label: &str,
    value: String,
    current: f64,
    peak: f64,
    scale_max: f64,
    color: Color,
) -> Vec<Span<'static>> {
    let theme = model.theme;
    let mut spans = vec![
        Span::styled(
            format!("{label} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{value} "),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    if model.advanced_ui {
        spans.extend(
            BrailleBar::new(current, scale_max)
                .peak(peak)
                .fill_color(color)
                .peak_color(theme.danger)
                .empty_color(theme.border)
                .into_line(METER_WIDTH)
                .spans,
        );
    } else {
        spans.push(Span::styled(
            ascii_meter(model, current, scale_max),
            Style::default().fg(color),
        ));
    }
    spans
}

/// ASCII stand-in for the braille meter, for terminals without Unicode.
///
/// Only reached on the `!advanced_ui` path, which is why routing it through
/// [`chrome::ratio_bar`] — whose glyphs are chosen per `advanced_ui` — still
/// draws the same `#`/`-` bar as the hand-rolled version did.
fn ascii_meter(model: &TuiModel, current: f64, scale_max: f64) -> String {
    let ratio = if scale_max > 0.0 {
        (current / scale_max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    chrome::ratio_bar(model, Some(ratio), METER_WIDTH)
}

/// The encoder's outgoing bitrate, or a same-shaped placeholder before OBS has
/// reported one. Spelled the same way by the full strip and its compact
/// single-line fallback.
fn bitrate_text(model: &TuiModel) -> String {
    model
        .stream_bitrate_kbps()
        .map(|value| format!("{value:.0}kbps"))
        .unwrap_or_else(|| "--kbps".to_string())
}

/// Highest value seen this session, never below the live one so the marker
/// can't sit behind the fill.
/// Round the peak up to a fixed step so the memory scale stays put between
/// samples instead of rescaling the bar under the reader.
fn memory_scale(peak_mb: f64) -> f64 {
    let steps = (peak_mb / MEMORY_SCALE_STEP_MB).ceil().max(1.0);
    steps * MEMORY_SCALE_STEP_MB
}

fn compact_metrics(model: &TuiModel, theme: Theme) -> Span<'static> {
    match model.stats() {
        Some(stats) => Span::styled(
            format!(
                "CPU {:.1}%  FPS {:.1}  MEM {:.0}MB  {}",
                stats.cpu_usage_percent,
                stats.active_fps,
                stats.memory_usage_mb,
                bitrate_text(model)
            ),
            Style::default().fg(theme.info),
        ),
        None => chrome::placeholder_span(
            model,
            chrome::typographic(model, &t!("tui.panels.live_bar.telemetry_waiting")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_scale_rounds_up_to_a_stable_step() {
        assert_eq!(memory_scale(0.0), 256.0);
        assert_eq!(memory_scale(100.0), 256.0);
        assert_eq!(memory_scale(256.0), 256.0);
        assert_eq!(memory_scale(257.0), 512.0);
        assert_eq!(memory_scale(1_100.0), 1_280.0);
    }

    #[test]
    fn memory_scale_never_collapses_to_zero() {
        // A zero/negative reading must not produce a zero divisor.
        assert!(memory_scale(0.0) > 0.0);
        assert!(memory_scale(-5.0) > 0.0);
    }

    #[test]
    fn ascii_meter_is_fixed_width_and_clamps() {
        // The ASCII meter is only reached with the advanced UI off.
        let model = TuiModel::with_appearance(TuiModel::default().theme, false, false);
        assert_eq!(ascii_meter(&model, 0.0, 100.0).chars().count(), METER_WIDTH);
        assert_eq!(ascii_meter(&model, 50.0, 100.0), "#####-----");
        assert_eq!(
            ascii_meter(&model, 100.0, 100.0).matches('#').count(),
            METER_WIDTH
        );
        // Over scale clamps rather than overflowing the strip.
        assert_eq!(
            ascii_meter(&model, 500.0, 100.0).matches('#').count(),
            METER_WIDTH
        );
        // A zero scale is survivable, not a divide-by-zero panic.
        assert_eq!(ascii_meter(&model, 5.0, 0.0).matches('#').count(), 0);
    }
}
