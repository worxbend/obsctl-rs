use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::tui::{anim, theme::Theme, widgets::chrome};

const TAGLINE: &str = "Broadcast control, without breaking flow.";
const APP_DESCRIPTION: &str =
    "Scenes, audio, profiles, recording, and live telemetry — one command center.";
const PROGRESS_BAR_WIDTH: usize = 30;
const LARGE_LOGO: &[&str] = &[
    " ██████╗ ██████╗ ███████╗ ██████╗████████╗██╗     ",
    "██╔═══██╗██╔══██╗██╔════╝██╔════╝╚══██╔══╝██║     ",
    "██║   ██║██████╔╝███████╗██║        ██║   ██║     ",
    "██║   ██║██╔══██╗╚════██║██║        ██║   ██║     ",
    "╚██████╔╝██████╔╝███████║╚██████╗   ██║   ███████╗",
    " ╚═════╝ ╚═════╝ ╚══════╝ ╚═════╝   ╚═╝   ╚══════╝",
];

pub fn render(f: &mut Frame, theme: Theme, frame: u64, total_frames: u64) {
    render_with_symbols(f, theme, frame, total_frames, true);
}

pub fn render_with_appearance(
    f: &mut Frame,
    theme: Theme,
    frame: u64,
    total_frames: u64,
    show_icons: bool,
    advanced_ui: bool,
) {
    if advanced_ui {
        render_with_symbols(f, theme, frame, total_frames, show_icons);
    } else {
        render_ascii(f, theme, frame, total_frames);
    }
}

/// Animated startup identity with a responsive large wordmark, moving color
/// wave, orbiting symbols, and a segmented multi-color boot indicator.
pub fn render_with_symbols(
    f: &mut Frame,
    theme: Theme,
    frame: u64,
    total_frames: u64,
    show_icons: bool,
) {
    let area = f.area();
    let large = area.width >= 76 && area.height >= 16;
    if large {
        render_large(f, theme, frame, total_frames, show_icons);
        return;
    }

    let desired_height = 10.min(area.height);
    let desired_width = 56.min(area.width);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(desired_height),
            Constraint::Min(0),
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(desired_width),
            Constraint::Min(0),
        ])
        .split(rows[1]);
    let card = cols[1];

    let pulse = ((frame as f32 * 0.18).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    let border = anim::blend(theme.accent, theme.accent_alt, pulse);
    let orbit = if show_icons {
        ["✦", "✧", "◆", "◇"][(frame as usize / 3) % 4]
    } else {
        ["*", "+", "o", "."][(frame as usize / 3) % 4]
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Line::from(vec![
            Span::styled(format!(" {orbit} "), Style::default().fg(theme.warning)),
            Span::styled(
                "INITIALIZING BROADCAST CONTROL",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {orbit} "), Style::default().fg(theme.info)),
        ]));
    let inner = block.inner(card);
    f.render_widget(block, card);

    let mut lines = vec![Line::raw("")];
    lines.push(
        anim::gradient_line(
            "◢ O B S C T L ◣",
            theme.accent,
            theme.accent_alt,
            frame,
            true,
        )
        .alignment(Alignment::Center),
    );

    lines.push(Line::raw(""));
    lines.push(Line::styled(TAGLINE, Style::default().fg(theme.fg)).alignment(Alignment::Center));
    lines.push(live_identity_line(frame, theme, show_icons));

    if inner.height >= 7 {
        lines.push(loader_line("SYNC", slither(frame, 18), theme.info, theme));
    }

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    lines.push(progress_line(progress, theme, frame).alignment(Alignment::Center));
    lines.push(
        Line::styled(
            boot_message(progress, show_icons),
            Style::default().fg(theme.muted),
        )
        .alignment(Alignment::Center),
    );

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_ascii(f: &mut Frame, theme: Theme, frame: u64, total_frames: u64) {
    const ASCII_LOGO: &[&str] = &[
        "  ___  ____  ____   ____ _____ _     ",
        " / _ \\| __ )/ ___| / ___|_   _| |    ",
        "| | | |  _ \\___ \\| |     | | | |    ",
        "| |_| | |_) |___) | |___  | | | |___ ",
        " \\___/|____/|____/ \\____| |_| |_____|",
    ];
    let area = f.area();
    let height = 13.min(area.height);
    let width = 64.min(area.width);
    let rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .split(area);
    let cols = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(width),
        Constraint::Min(0),
    ])
    .split(rows[1]);
    let card = Block::default()
        .borders(Borders::ALL)
        .border_set(chrome::ASCII_BORDER)
        .border_style(Style::default().fg(theme.accent))
        .title(" OBSCTL STARTUP ");
    let inner = card.inner(cols[1]);
    f.render_widget(card, cols[1]);

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    let filled = (progress * 30.0).round() as usize;
    let bar = format!(
        "[{}{}] {:>3}%",
        "#".repeat(filled),
        ".".repeat(30usize.saturating_sub(filled)),
        (progress * 100.0).round() as u8
    );
    let spinner = ["|", "/", "-", "\\"][(frame as usize / 2) % 4];
    let mut lines = ASCII_LOGO
        .iter()
        .map(|line| {
            Line::styled(
                *line,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(Alignment::Center)
        })
        .collect::<Vec<_>>();
    lines.extend([
        Line::styled(TAGLINE, Style::default().fg(theme.fg)).alignment(Alignment::Center),
        Line::styled(
            "Scenes, audio, profiles, recording, and live telemetry.",
            Style::default().fg(theme.muted),
        )
        .alignment(Alignment::Center),
        Line::styled(
            format!("{spinner}  (o) LIVE  {spinner}"),
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
        Line::raw(""),
        Line::styled(bar, Style::default().fg(theme.info)).alignment(Alignment::Center),
        Line::styled(
            format!("{spinner} initializing studio link"),
            Style::default().fg(theme.muted),
        )
        .alignment(Alignment::Center),
    ]);
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_large(f: &mut Frame, theme: Theme, frame: u64, total_frames: u64, show_icons: bool) {
    let area = f.area();
    let stage_height = 16.min(area.height);
    let stage_width = 88.min(area.width);
    let stage_rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(stage_height),
        Constraint::Min(0),
    ])
    .split(area);
    let stage_cols = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(stage_width),
        Constraint::Min(0),
    ])
    .split(stage_rows[1]);
    let stage = stage_cols[1];
    let sections = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(3),
        Constraint::Length(7),
    ])
    .split(stage);

    let logo = LARGE_LOGO
        .iter()
        .enumerate()
        .map(|(row, text)| {
            anim::gradient_line(
                text,
                theme.accent,
                theme.accent_alt,
                frame + row as u64 * 2,
                true,
            )
            .alignment(Alignment::Center)
        })
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(logo), sections[0]);

    f.render_widget(
        Paragraph::new(vec![
            anim::gradient_line(TAGLINE, theme.accent_alt, theme.info, frame, true)
                .alignment(Alignment::Center),
            Line::styled(APP_DESCRIPTION, Style::default().fg(theme.muted))
                .alignment(Alignment::Center),
            live_identity_line(frame, theme, show_icons),
        ]),
        sections[1],
    );

    let pulse = ((frame as f32 * 0.18).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    let rail_color = anim::blend(theme.accent, theme.accent_alt, pulse);
    let card = Block::default()
        .borders(Borders::LEFT)
        .border_type(BorderType::Thick)
        .border_style(Style::default().fg(rail_color))
        .style(Style::default().bg(anim::blend(theme.bg, theme.border, 0.34)))
        .title(Line::styled(
            " STUDIO LINK // SECURE SESSION ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = card.inner(sections[2]);
    f.render_widget(card, sections[2]);

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    let lines = vec![
        loader_line("SIGNAL", slither(frame, 32), theme.accent, theme),
        loader_line("LIQUID", liquid_wave(frame, 32), theme.accent_alt, theme),
        Line::raw(""),
        progress_line(progress, theme, frame).alignment(Alignment::Center),
        Line::styled(
            boot_message(progress, show_icons),
            Style::default().fg(theme.muted),
        )
        .alignment(Alignment::Center),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn live_identity_line(frame: u64, theme: Theme, show_icons: bool) -> Line<'static> {
    let phase = (frame as usize / 2) % 4;
    let icon = if show_icons {
        ["◉", "●", "◍", "○"][phase]
    } else {
        ["O", "o", "O", "."][phase]
    };
    let wings = if show_icons {
        [("⟫", "⟪"), ("›", "‹"), ("·", "·"), ("›", "‹")][phase]
    } else {
        [(">", "<"), ("-", "-"), (".", "."), ("-", "-")][phase]
    };
    let pulse = ((frame as f32 * 0.35).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    let color = anim::blend(theme.danger, theme.warning, pulse * 0.35);
    let badge_style = if phase.is_multiple_of(2) {
        Style::default()
            .fg(theme.bg)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    };

    Line::from(vec![
        Span::styled(format!("{}  ", wings.0), Style::default().fg(theme.muted)),
        Span::styled(format!(" {icon}  LIVE "), badge_style),
        Span::styled(format!("  {}", wings.1), Style::default().fg(theme.muted)),
    ])
    .alignment(Alignment::Center)
}

fn loader_line(
    label: &str,
    animation: String,
    color: ratatui::style::Color,
    theme: Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<6} "), Style::default().fg(theme.muted)),
        Span::styled(animation, Style::default().fg(color)),
    ])
    .alignment(Alignment::Center)
}

fn slither(frame: u64, width: usize) -> String {
    let mut cells = vec!['·'; width];
    if width == 0 {
        return String::new();
    }
    let head = (frame as usize) % width;
    for (offset, glyph) in ['█', '▓', '▒', '░'].into_iter().enumerate() {
        cells[(head + width - offset) % width] = glyph;
    }
    cells.into_iter().collect()
}

fn liquid_wave(frame: u64, width: usize) -> String {
    const LEVELS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    (0..width)
        .map(|index| {
            let phase = index as f32 * 0.62 + frame as f32 * 0.32;
            let value = phase.sin() * 0.5 + 0.5;
            LEVELS[(value * (LEVELS.len() - 1) as f32).round() as usize]
        })
        .collect()
}

fn progress_line(progress: f32, theme: Theme, frame: u64) -> Line<'static> {
    let filled = (progress * PROGRESS_BAR_WIDTH as f32).round() as usize;
    let mut spans = vec![Span::styled("  ", Style::default())];
    for index in 0..PROGRESS_BAR_WIDTH {
        if index < filled {
            let t = index as f32 / PROGRESS_BAR_WIDTH as f32;
            let color = anim::blend(theme.accent, theme.accent_alt, t);
            spans.push(Span::styled("█", Style::default().fg(color)));
        } else if index == filled && filled < PROGRESS_BAR_WIDTH {
            let cursor = if frame.is_multiple_of(2) {
                "▓"
            } else {
                "▒"
            };
            spans.push(Span::styled(cursor, Style::default().fg(theme.info)));
        } else {
            spans.push(Span::styled("░", Style::default().fg(theme.border)));
        }
    }
    spans.push(Span::styled(
        format!("  {:>3}%", (progress * 100.0).round() as u8),
        Style::default()
            .fg(theme.warning)
            .add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

fn boot_message(progress: f32, show_icons: bool) -> String {
    let (icon, message) = if progress < 0.25 {
        ("◌", "loading control surfaces")
    } else if progress < 0.5 {
        ("◍", "warming animation engine")
    } else if progress < 0.75 {
        ("◉", "syncing OBS telemetry")
    } else if progress < 1.0 {
        ("◎", "painting command center")
    } else {
        ("✓", "ready")
    };
    if show_icons {
        format!("{icon}  {message}")
    } else {
        format!("[..] {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaders_keep_a_stable_width_while_animating() {
        assert_eq!(slither(0, 18).chars().count(), 18);
        assert_eq!(slither(7, 18).chars().count(), 18);
        assert_ne!(slither(0, 18), slither(1, 18));
        assert_eq!(liquid_wave(3, 24).chars().count(), 24);
    }

    #[test]
    fn live_identity_animates_across_frames() {
        let theme = Theme::default_theme();
        assert_ne!(
            format!("{:?}", live_identity_line(0, theme, true)),
            format!("{:?}", live_identity_line(2, theme, true))
        );
    }
}
