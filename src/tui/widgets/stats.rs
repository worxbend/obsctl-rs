//! Stream health pane — the frame-level telemetry from `GetStats` that tells
//! a streamer whether OBS is actually keeping up: output FPS, how long the
//! average frame takes to render, and the frames the render and output
//! pipelines are losing.
//!
//! It sits beside the logs strip and only while streaming (see
//! `tui::layout::compute`), since these numbers are only actionable on air.
//! Every row is colored by health rather than just printed, so overload is
//! visible from across a room without reading the digits.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::tui::{
    anim,
    model::{FrameCounters, TuiModel},
    theme::Theme,
    widgets::chrome,
};

/// Width of the inline sparkline / budget bar on the FPS and frame-time rows.
const GRAPH_WIDTH: usize = 10;

/// Share of the per-frame time budget that counts as comfortable, then as
/// strained. Past the second threshold OBS is close to missing its frame
/// deadline and the compositor starts dropping.
const FRAME_BUDGET_OK: f64 = 0.5;
const FRAME_BUDGET_WARN: f64 = 0.8;

/// Fraction of the target FPS that still counts as holding the framerate.
const FPS_OK: f64 = 0.95;
const FPS_WARN: f64 = 0.85;

/// Drop rates (skipped / total) that separate a clean stream from a
/// degrading one. Anything past 1% is visible to viewers as stutter.
const DROP_OK: f64 = 0.001;
const DROP_WARN: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Health {
    Good,
    Strained,
    Bad,
    /// Not enough data yet to judge (no samples, or a zero denominator).
    Unknown,
}

impl Health {
    fn color(self, theme: Theme) -> Color {
        match self {
            Self::Good => theme.success,
            Self::Strained => theme.warning,
            Self::Bad => theme.danger,
            Self::Unknown => theme.muted,
        }
    }

    /// Worst of the two, treating `Unknown` as "no opinion" so a metric that
    /// hasn't reported yet never drags the verdict down.
    fn worse(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, other) => other,
            (this, Self::Unknown) => this,
            (this, other) => this.max(other),
        }
    }
}

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let stats = model.stats().copied();

    let block = chrome::panel(
        model.symbol("⚡", "%"),
        "Stats // Stream Health",
        "frames",
        stats.map(|s| s.active_fps.round() as usize).unwrap_or(0),
        false,
        model,
    );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(stats) = stats else {
        f.render_widget(
            Paragraph::new(Line::styled(
                if model.advanced_ui {
                    " waiting for OBS metrics…"
                } else {
                    " waiting for OBS metrics..."
                },
                Style::default().fg(theme.muted),
            )),
            inner,
        );
        return;
    };

    // Per-stream counters when we have a baseline for the current
    // broadcast, otherwise OBS's since-launch totals — labelled either way
    // so the numbers are never ambiguous.
    let (counters, scope) = match model.stream_frame_drops() {
        Some(drops) => (drops, "this stream"),
        None => (FrameCounters::from_stats(&stats), "since launch"),
    };

    let target_fps = target_fps(model, stats.active_fps);
    let fps_health = fps_health(stats.active_fps, target_fps);
    let frame_usage = frame_budget_usage(stats.average_frame_render_time_ms, stats.active_fps);
    let frame_health = frame_health(frame_usage);
    let render_rate = drop_rate(counters.render_skipped, counters.render_total);
    let output_rate = drop_rate(counters.output_skipped, counters.output_total);
    let render_health = drop_health(render_rate);
    let output_health = drop_health(output_rate);

    let lines = vec![
        fps_line(model, stats.active_fps, target_fps, fps_health),
        frame_time_line(
            model,
            stats.average_frame_render_time_ms,
            frame_usage,
            frame_health,
        ),
        drop_line(
            model,
            "RENDER",
            counters.render_skipped,
            counters.render_total,
            render_rate,
            render_health,
            scope,
        ),
        drop_line(
            model,
            "OUTPUT",
            counters.output_skipped,
            counters.output_total,
            output_rate,
            output_health,
            scope,
        ),
        verdict_line(
            model,
            fps_health
                .worse(frame_health)
                .worse(render_health)
                .worse(output_health),
        ),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}

fn fps_line(model: &TuiModel, fps: f64, target: f64, health: Health) -> Line<'static> {
    let theme = model.theme;
    let graph = if model.advanced_ui {
        anim::sparkline(model.fps_history.samples(), GRAPH_WIDTH)
    } else {
        anim::sparkline_ascii(model.fps_history.samples(), GRAPH_WIDTH)
    };
    Line::from(vec![
        label(model, "FPS"),
        value(format!("{fps:>7.1}"), health.color(theme)),
        Span::styled(format!("  {graph}"), Style::default().fg(theme.info)),
        Span::styled(
            format!("  target {target:.0}"),
            Style::default().fg(theme.muted),
        ),
    ])
}

fn frame_time_line(
    model: &TuiModel,
    render_ms: f64,
    usage: Option<f64>,
    health: Health,
) -> Line<'static> {
    let theme = model.theme;
    let bar = budget_bar(model, usage);
    let trailing = match usage {
        Some(usage) => format!("  {:.0}% budget", usage * 100.0),
        None => "  -- budget".to_string(),
    };
    Line::from(vec![
        label(model, "FRAME"),
        value(format!("{render_ms:>5.2}ms"), health.color(theme)),
        Span::styled(format!("  {bar}"), Style::default().fg(health.color(theme))),
        Span::styled(trailing, Style::default().fg(theme.muted)),
    ])
}

fn drop_line(
    model: &TuiModel,
    name: &'static str,
    skipped: u64,
    total: u64,
    rate: Option<f64>,
    health: Health,
    scope: &'static str,
) -> Line<'static> {
    let theme = model.theme;
    let percent = match rate {
        Some(rate) => format!("{:>6.2}%", rate * 100.0),
        None => "    --%".to_string(),
    };
    let counts = format!(
        "{:<13}",
        format!("{}/{}", compact_count(skipped), compact_count(total))
    );
    Line::from(vec![
        label(model, name),
        value(percent, health.color(theme)),
        Span::styled(
            format!("  {counts}"),
            Style::default().fg(if skipped == 0 { theme.muted } else { theme.fg }),
        ),
        Span::styled(scope.to_string(), Style::default().fg(theme.muted)),
    ])
}

fn verdict_line(model: &TuiModel, health: Health) -> Line<'static> {
    let theme = model.theme;
    let (icon, headline, detail) = match health {
        Health::Good => (
            model.symbol("✔", "+"),
            "HEALTHY",
            "encoder keeping up with OBS",
        ),
        Health::Strained => (
            model.symbol("▲", "!"),
            "STRAINED",
            "frames are starting to slip",
        ),
        Health::Bad => (
            model.symbol("◆", "x"),
            "DROPPING",
            "reduce bitrate or scene load",
        ),
        Health::Unknown => (model.symbol("·", "."), "MEASURING", "collecting samples"),
    };
    // The verdict is the one row that should catch the eye when it turns
    // bad, so a degraded stream pulses in time with the LIVE badge.
    let color = if health == Health::Bad && model.advanced_ui {
        anim::blend(theme.danger, theme.warning, model.anim.pulse(24) * 0.5)
    } else {
        health.color(theme)
    };
    Line::from(vec![
        Span::styled(
            format!(" {icon} {headline}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {detail}"),
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

fn label(model: &TuiModel, text: &str) -> Span<'static> {
    Span::styled(
        format!(" {text:<7}"),
        Style::default()
            .fg(model.theme.accent_alt)
            .add_modifier(Modifier::BOLD),
    )
}

fn value(text: String, color: Color) -> Span<'static> {
    Span::styled(
        text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// A filled/empty bar showing how much of the per-frame time budget the
/// average render is using.
fn budget_bar(model: &TuiModel, usage: Option<f64>) -> String {
    let (full, empty) = if model.advanced_ui {
        ('█', '░')
    } else {
        ('#', '-')
    };
    let Some(usage) = usage else {
        return std::iter::repeat_n(empty, GRAPH_WIDTH).collect();
    };
    let filled = (usage.clamp(0.0, 1.0) * GRAPH_WIDTH as f64).round() as usize;
    std::iter::repeat_n(full, filled)
        .chain(std::iter::repeat_n(empty, GRAPH_WIDTH - filled))
        .collect()
}

/// OBS reports the FPS it is currently achieving, never the one it is
/// configured for, so the best available reference is the highest rate this
/// session has actually sustained.
fn target_fps(model: &TuiModel, current: f64) -> f64 {
    model.fps_history.peak(current)
}

fn fps_health(current: f64, target: f64) -> Health {
    if target <= 0.0 {
        return Health::Unknown;
    }
    match current / target {
        ratio if ratio >= FPS_OK => Health::Good,
        ratio if ratio >= FPS_WARN => Health::Strained,
        _ => Health::Bad,
    }
}

/// Fraction of the frame's time budget (1s / FPS) spent rendering it.
fn frame_budget_usage(render_ms: f64, fps: f64) -> Option<f64> {
    (fps > 0.0 && render_ms >= 0.0).then(|| render_ms / (1000.0 / fps))
}

fn frame_health(usage: Option<f64>) -> Health {
    match usage {
        None => Health::Unknown,
        Some(usage) if usage < FRAME_BUDGET_OK => Health::Good,
        Some(usage) if usage < FRAME_BUDGET_WARN => Health::Strained,
        Some(_) => Health::Bad,
    }
}

fn drop_rate(skipped: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| skipped as f64 / total as f64)
}

fn drop_health(rate: Option<f64>) -> Health {
    match rate {
        None => Health::Unknown,
        Some(rate) if rate <= DROP_OK => Health::Good,
        Some(rate) if rate <= DROP_WARN => Health::Strained,
        Some(_) => Health::Bad,
    }
}

/// Frame counters reach millions on a long session; keep them to a width the
/// pane can actually fit.
fn compact_count(value: u64) -> String {
    match value {
        0..=9_999 => value.to_string(),
        10_000..=999_999 => format!("{:.1}k", value as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.1}M", value as f64 / 1_000_000.0),
        _ => format!("{:.1}G", value as f64 / 1_000_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_budget_usage_is_render_time_over_the_frame_interval() {
        // 60fps => 16.67ms budget; an 8.33ms render uses half of it.
        let usage = frame_budget_usage(8.333, 60.0).unwrap();
        assert!((usage - 0.5).abs() < 0.01, "unexpected usage {usage}");
        assert_eq!(frame_budget_usage(5.0, 0.0), None);
    }

    #[test]
    fn frame_health_tracks_the_budget_thresholds() {
        assert_eq!(frame_health(Some(0.2)), Health::Good);
        assert_eq!(frame_health(Some(0.6)), Health::Strained);
        assert_eq!(frame_health(Some(0.95)), Health::Bad);
        assert_eq!(frame_health(None), Health::Unknown);
    }

    #[test]
    fn fps_health_compares_against_the_sustained_target() {
        assert_eq!(fps_health(60.0, 60.0), Health::Good);
        assert_eq!(fps_health(53.0, 60.0), Health::Strained);
        assert_eq!(fps_health(30.0, 60.0), Health::Bad);
        assert_eq!(fps_health(0.0, 0.0), Health::Unknown);
    }

    #[test]
    fn drop_health_flags_visible_stutter() {
        assert_eq!(drop_health(drop_rate(0, 10_000)), Health::Good);
        assert_eq!(drop_health(drop_rate(50, 10_000)), Health::Strained);
        assert_eq!(drop_health(drop_rate(500, 10_000)), Health::Bad);
        // No frames rendered yet is not a failure.
        assert_eq!(drop_health(drop_rate(0, 0)), Health::Unknown);
    }

    #[test]
    fn verdict_takes_the_worst_metric_and_ignores_unknowns() {
        assert_eq!(Health::Good.worse(Health::Bad), Health::Bad);
        assert_eq!(Health::Strained.worse(Health::Good), Health::Strained);
        assert_eq!(Health::Unknown.worse(Health::Good), Health::Good);
        assert_eq!(Health::Good.worse(Health::Unknown), Health::Good);
        assert_eq!(Health::Unknown.worse(Health::Unknown), Health::Unknown);
    }

    #[test]
    fn target_fps_uses_the_best_rate_seen_this_session() {
        let mut model = TuiModel::default();
        model.fps_history = [60.0, 59.9, 44.0].into_iter().collect();
        assert_eq!(target_fps(&model, 44.0), 60.0);
        // A brand new session with no history falls back to the live value.
        assert_eq!(target_fps(&TuiModel::default(), 30.0), 30.0);
    }

    #[test]
    fn budget_bar_fills_proportionally_and_stays_fixed_width() {
        let model = TuiModel::default();
        assert_eq!(budget_bar(&model, Some(0.0)).chars().count(), GRAPH_WIDTH);
        assert_eq!(budget_bar(&model, Some(0.5)).chars().count(), GRAPH_WIDTH);
        assert_eq!(budget_bar(&model, None).chars().count(), GRAPH_WIDTH);
        assert_eq!(budget_bar(&model, Some(0.5)).matches('█').count(), 5);
        // Overrun clamps instead of overflowing the pane.
        assert_eq!(
            budget_bar(&model, Some(3.0)).matches('█').count(),
            GRAPH_WIDTH
        );
    }

    #[test]
    fn budget_bar_uses_ascii_glyphs_in_simple_mode() {
        let mut model = TuiModel::default();
        model.advanced_ui = false;
        let bar = budget_bar(&model, Some(0.5));
        assert_eq!(bar, "#####-----");
    }

    #[test]
    fn compact_count_keeps_large_counters_narrow() {
        assert_eq!(compact_count(0), "0");
        assert_eq!(compact_count(9_999), "9999");
        assert_eq!(compact_count(10_000), "10.0k");
        assert_eq!(compact_count(1_260_000), "1.3M");
        // A century of 60fps — comfortably past any real counter — still
        // fits the fixed-width counts column.
        let a_century_at_60fps = 60 * 86_400 * 365 * 100;
        assert_eq!(compact_count(a_century_at_60fps), "189.2G");
        assert!(compact_count(a_century_at_60fps).len() <= 13);
    }
}
