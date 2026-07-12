use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::tui::{anim, theme::Theme};

const TAGLINE: &str = "OBS Studio Controller";
/// Letters of the wordmark, spelled out individually so each can carry its
/// own animated color phase (a per-letter shimmer/marquee sweep).
const LETTERS: &[char] = &['O', 'B', 'S', ' ', 'c', 't', 'l'];
const PROGRESS_BAR_WIDTH: usize = 24;

/// Render one frame of the startup splash. `frame`/`total_frames` drive
/// both the letter shimmer and the loading bar fill.
pub fn render(f: &mut Frame, theme: Theme, frame: u64, total_frames: u64) {
    let area = f.area();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .split(area);
    let band = rows[1];

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(area.width.min(50)),
            Constraint::Min(0),
        ])
        .split(band);
    let inner_area = cols[1];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focus));
    let inner = block.inner(inner_area);
    f.render_widget(block, inner_area);

    let logo_spans: Vec<_> = LETTERS
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let phase = frame as f32 * 0.35 + i as f32 * 0.9;
            let t = phase.sin() * 0.5 + 0.5;
            let color = anim::blend(theme.fg, theme.accent, t);
            ratatui::text::Span::styled(
                format!("{c} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    let filled = (progress * PROGRESS_BAR_WIDTH as f32).round() as usize;
    let filled = filled.min(PROGRESS_BAR_WIDTH);
    let bar = format!(
        "[{}{}]",
        "█".repeat(filled),
        "░".repeat(PROGRESS_BAR_WIDTH - filled)
    );

    let lines = vec![
        Line::raw(""),
        Line::from(logo_spans).alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(TAGLINE, Style::default().fg(theme.muted)).alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(bar, Style::default().fg(theme.accent)).alignment(Alignment::Center),
    ];

    f.render_widget(Paragraph::new(lines), inner);
}
