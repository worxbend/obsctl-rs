use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, Borders, List, ListItem},
};

use crate::{
    ipc::protocol::LogLevel,
    tui::{
        model::{TuiLogEntry, TuiModel},
        theme::Theme,
    },
};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let theme = model.theme;
    let height = area.height.saturating_sub(2) as usize;
    let skip = model.logs.len().saturating_sub(height);

    let items: Vec<ListItem> = model
        .logs
        .iter()
        .skip(skip)
        .map(|entry| {
            let style = style_for_level(entry.level, theme);
            ListItem::new(Line::styled(format_log_entry(entry), style))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(" Logs ");
    f.render_widget(List::new(items).block(block), area);
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

fn format_log_entry(entry: &TuiLogEntry) -> String {
    let time = format!(
        "{:02}:{:02}:{:02}",
        entry.timestamp.hour(),
        entry.timestamp.minute(),
        entry.timestamp.second()
    );
    let level = level_label(entry.level);

    match entry.target.as_deref() {
        Some(target) if !target.is_empty() => {
            format!("{time} {level:<5} {target}: {}", entry.message)
        }
        _ => format!("{time} {level:<5} {}", entry.message),
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
