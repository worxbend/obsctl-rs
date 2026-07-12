use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::{anim, model::TuiModel, theme::Theme};

/// Ticks for one full pulse cycle of the LIVE/REC badges.
const PULSE_PERIOD_TICKS: u64 = 24;
/// How far the pulse dims the badge color toward the theme foreground
/// (kept subtle — this is a glow, not a strobe).
const PULSE_DEPTH: f32 = 0.4;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let streaming = model
        .snapshot
        .as_ref()
        .map(|s| s.streaming)
        .unwrap_or(false);
    let recording = model
        .snapshot
        .as_ref()
        .map(|s| s.recording)
        .unwrap_or(false);
    let pulse = model.anim.pulse(PULSE_PERIOD_TICKS);

    let live_span = badge("LIVE", streaming, model.stream_duration_ms(), pulse, theme);
    let rec_span = badge("REC", recording, model.record_duration_ms(), pulse, theme);

    let line = Line::from(vec![
        live_span,
        Span::raw("   "),
        rec_span,
        Span::raw("    "),
        stats_span(model, theme),
    ]);

    let active = streaming || recording;
    let border_color = if active {
        anim::blend(theme.border, theme.danger, pulse)
    } else {
        theme.border
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(" Status ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(line), inner);
}

fn badge(
    label: &str,
    active: bool,
    duration_ms: Option<u64>,
    pulse: f32,
    theme: Theme,
) -> Span<'static> {
    if !active {
        return Span::styled(format!(" ○ {label} off "), Style::default().fg(theme.muted));
    }
    let color = anim::blend(theme.danger, theme.fg, pulse * PULSE_DEPTH);
    let dot = if pulse > 0.5 { "●" } else { "◉" };
    let duration = format_duration(duration_ms);
    Span::styled(
        format!(" {dot} {label}  {duration} "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn stats_span(model: &TuiModel, theme: Theme) -> Span<'static> {
    match model.stats() {
        Some(stats) => {
            let bitrate = model
                .stream_bitrate_kbps()
                .map(|kbps| format!("{kbps:.0} kbps"))
                .unwrap_or_else(|| "-- kbps".to_string());
            Span::styled(
                format!(
                    "CPU {:.1}%  FPS {:.1}  MEM {:.0}MB  {bitrate}",
                    stats.cpu_usage_percent, stats.active_fps, stats.memory_usage_mb,
                ),
                Style::default().fg(theme.info),
            )
        }
        None => Span::styled("stats: waiting…", Style::default().fg(theme.muted)),
    }
}

fn format_duration(ms: Option<u64>) -> String {
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
