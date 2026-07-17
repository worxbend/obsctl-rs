use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::tui::widgets::chrome;
use crate::tui::{anim, model::TuiModel, theme::Theme};

const PULSE_PERIOD_TICKS: u64 = 24;

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let streaming = model.snapshot.as_ref().is_some_and(|s| s.streaming);
    let recording = model.snapshot.as_ref().is_some_and(|s| s.recording);
    let pulse = model.anim.pulse(PULSE_PERIOD_TICKS);
    let active = streaming || recording;
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

    let live = badge("LIVE", streaming, model.stream_duration_ms(), pulse, model);
    let rec = badge("REC", recording, model.record_duration_ms(), pulse, model);
    if inner.height < 2 {
        let compact = Line::from(vec![
            live,
            Span::raw("  "),
            rec,
            separator(model, theme),
            compact_metrics(model, theme),
        ]);
        f.render_widget(Paragraph::new(compact), inner);
        return;
    }

    let first = Line::from(vec![
        live,
        Span::raw("  "),
        rec,
        separator(model, theme),
        Span::styled(
            format!(
                "{} {}",
                model.symbol("🎬", "SCN"),
                model.current_scene().unwrap_or("no active scene")
            ),
            Style::default().fg(theme.fg),
        ),
    ]);

    let second = metrics_line(model, theme);
    f.render_widget(Paragraph::new(vec![first, second]), inner);
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

fn badge(
    label: &str,
    active: bool,
    duration_ms: Option<u64>,
    pulse: f32,
    model: &TuiModel,
) -> Span<'static> {
    let theme = model.theme;
    if !active {
        return Span::styled(
            format!(" {} {label} off ", model.symbol("○", "-")),
            Style::default().fg(theme.muted),
        );
    }
    let color = anim::blend(theme.danger, theme.warning, pulse * 0.45);
    let dot = if pulse > 0.5 {
        model.symbol("●", "*")
    } else {
        model.symbol("◉", "*")
    };
    Span::styled(
        format!(" {dot} {label} {} ", format_duration(duration_ms)),
        Style::default()
            .fg(color)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
    )
}

fn metrics_line(model: &TuiModel, theme: Theme) -> Line<'static> {
    let Some(stats) = model.stats() else {
        return Line::from(vec![
            Span::styled(
                format!("{} telemetry", model.symbol("⌁", "~")),
                Style::default().fg(theme.info),
            ),
            Span::styled(
                if model.advanced_ui {
                    "  waiting for OBS metrics…"
                } else {
                    "  waiting for OBS metrics..."
                },
                Style::default().fg(theme.muted),
            ),
        ]);
    };
    let graph = if model.advanced_ui {
        anim::sparkline
    } else {
        anim::sparkline_ascii
    };
    let cpu_graph = graph(&model.cpu_history, 10);
    let bitrate_graph = graph(&model.bitrate_history, 10);
    let bitrate = model
        .stream_bitrate_kbps()
        .map(|value| format!("{value:.0}kbps"))
        .unwrap_or_else(|| "--kbps".to_string());
    Line::from(vec![
        metric(
            "CPU",
            &format!("{:.1}%", stats.cpu_usage_percent),
            &cpu_graph,
            theme.warning,
        ),
        separator(model, theme),
        metric(
            "FPS",
            &format!("{:.1}", stats.active_fps),
            "",
            theme.success,
        ),
        separator(model, theme),
        metric(
            "MEM",
            &format!("{:.0}MB", stats.memory_usage_mb),
            "",
            theme.info,
        ),
        separator(model, theme),
        metric("NET", &bitrate, &bitrate_graph, theme.accent_alt),
    ])
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

fn metric(label: &str, value: &str, graph: &str, color: ratatui::style::Color) -> Span<'static> {
    let suffix = if graph.is_empty() {
        String::new()
    } else {
        format!(" {graph}")
    };
    Span::styled(
        format!("{label} {value}{suffix}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
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
