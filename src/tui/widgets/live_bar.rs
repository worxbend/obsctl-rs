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
    widgets::{Block, BorderType, Borders, Paragraph},
};
use ratatui_braille_bar::BrailleBar;

use crate::tui::widgets::chrome;
use crate::tui::{anim, model::TuiModel, spinner, theme::Theme};

const PULSE_PERIOD_TICKS: u64 = 24;

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
    let pulse = model.anim.pulse(PULSE_PERIOD_TICKS);
    let active = model.streaming() || model.recording();
    let border = if !model.advanced_ui {
        theme.border
    } else if active {
        anim::blend(theme.border, theme.danger, pulse * 0.8)
    } else {
        anim::blend(theme.border, theme.info, pulse * 0.18)
    };
    let title = if model.advanced_ui {
        anim::gradient_line(
            " ◉ LIVE TELEMETRY ",
            theme.danger,
            theme.warning,
            model.anim.frame,
            true,
        )
    } else {
        Line::styled(
            " LIVE TELEMETRY ",
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(title);
    let block = if model.advanced_ui {
        block
    } else {
        block.border_set(chrome::ASCII_BORDER)
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut first = Vec::new();
    if include_badges {
        first.extend(badges(model, pulse));
        first.push(separator(model, theme));
    }
    first.push(Span::styled(
        format!(
            "{} {}",
            model.symbol("🎬", "SCN"),
            model.current_scene().unwrap_or("no active scene")
        ),
        Style::default().fg(theme.fg),
    ));

    if inner.height < 2 {
        first.push(separator(model, theme));
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
            separator(model, theme),
            Span::styled(
                if model.advanced_ui {
                    "waiting for OBS metrics…"
                } else {
                    "waiting for OBS metrics..."
                },
                Style::default().fg(theme.muted),
            ),
        ];
    };
    let graph = if model.advanced_ui {
        anim::sparkline
    } else {
        anim::sparkline_ascii
    };
    let bitrate = model
        .stream_bitrate_kbps()
        .map(|value| format!("{value:.0}kbps"))
        .unwrap_or_else(|| "--kbps".to_string());
    vec![
        separator(model, theme),
        Span::styled(
            format!("FPS {:.1}", stats.active_fps),
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::BOLD),
        ),
        separator(model, theme),
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
        return Line::from(vec![Span::styled(
            format!(
                "{} {}",
                model.symbol("⌁", "~"),
                if model.advanced_ui {
                    "telemetry idle — no CPU or memory samples yet"
                } else {
                    "telemetry idle - no CPU or memory samples yet"
                }
            ),
            Style::default().fg(theme.muted),
        )]);
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
    spans.push(separator(model, theme));
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
            ascii_meter(current, scale_max),
            Style::default().fg(color),
        ));
    }
    spans
}

/// ASCII stand-in for the braille meter, for terminals without Unicode.
fn ascii_meter(current: f64, scale_max: f64) -> String {
    let ratio = if scale_max > 0.0 {
        (current / scale_max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (ratio * METER_WIDTH as f64).round() as usize;
    std::iter::repeat_n('#', filled)
        .chain(std::iter::repeat_n('-', METER_WIDTH - filled))
        .collect()
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
                model
                    .stream_bitrate_kbps()
                    .map(|value| format!("{value:.0}kbps"))
                    .unwrap_or_else(|| "--kbps".to_string())
            ),
            Style::default().fg(theme.info),
        ),
        None => Span::styled(
            if model.advanced_ui {
                "telemetry waiting…"
            } else {
                "telemetry waiting..."
            },
            Style::default().fg(theme.muted),
        ),
    }
}

fn separator(model: &TuiModel, theme: Theme) -> Span<'static> {
    Span::styled(
        if model.advanced_ui {
            "  │  "
        } else {
            "  |  "
        },
        Style::default().fg(theme.border),
    )
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
        assert_eq!(ascii_meter(0.0, 100.0).chars().count(), METER_WIDTH);
        assert_eq!(ascii_meter(50.0, 100.0), "#####-----");
        assert_eq!(ascii_meter(100.0, 100.0).matches('#').count(), METER_WIDTH);
        // Over scale clamps rather than overflowing the strip.
        assert_eq!(ascii_meter(500.0, 100.0).matches('#').count(), METER_WIDTH);
        // A zero scale is survivable, not a divide-by-zero panic.
        assert_eq!(ascii_meter(5.0, 0.0).matches('#').count(), 0);
    }
}
