use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{List, ListItem},
};

use crate::{
    ipc::protocol::LogLevel,
    tui::{
        model::{TuiLogEntry, TuiModel},
        theme::Theme,
        widgets::chrome,
    },
};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let height = area.height.saturating_sub(2) as usize;
    let skip = model.logs.len().saturating_sub(height);

    let items: Vec<ListItem> = model
        .logs
        .iter()
        .skip(skip)
        .map(|entry| ListItem::new(log_line(entry, model)))
        .collect();

    let block = chrome::panel(
        model.symbol("📡", "L"),
        "Logs // Event Stream",
        "live daemon feed",
        model.logs.len(),
        false,
        model,
    );
    f.render_widget(List::new(items).block(block), area);
}

fn log_line(entry: &TuiLogEntry, model: &TuiModel) -> Line<'static> {
    let theme = model.theme;
    let time = format!(
        "{:02}:{:02}:{:02}",
        entry.timestamp.hour(),
        entry.timestamp.minute(),
        entry.timestamp.second()
    );
    let marker = match entry.level {
        LogLevel::Trace => model.symbol("·", "."),
        LogLevel::Debug => model.symbol("◦", "."),
        LogLevel::Info => model.symbol("●", "i"),
        LogLevel::Warn => model.symbol("▲", "!"),
        LogLevel::Error => model.symbol("◆", "x"),
    };
    let style = style_for_level(entry.level, theme);
    let mut spans = vec![
        Span::styled(format!(" {marker} "), style),
        Span::styled(time, Style::default().fg(theme.muted)),
        Span::styled(
            format!(" {:<5} ", level_label(entry.level)),
            style.add_modifier(ratatui::style::Modifier::BOLD),
        ),
    ];
    if let Some(target) = entry.target.as_deref().filter(|target| !target.is_empty()) {
        spans.push(Span::styled(
            format!("{target}  "),
            Style::default().fg(theme.accent_alt),
        ));
    }
    spans.push(Span::styled(
        entry.message.clone(),
        Style::default().fg(theme.fg),
    ));
    Line::from(spans)
}

fn style_for_level(level: LogLevel, theme: Theme) -> Style {
    match level {
        LogLevel::Trace => Style::default().fg(theme.muted),
        LogLevel::Debug => Style::default().fg(theme.muted),
        LogLevel::Info => Style::default().fg(theme.fg),
        LogLevel::Warn => Style::default().fg(theme.warning),
        LogLevel::Error => Style::default().fg(theme.danger),
    }
}

fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}
