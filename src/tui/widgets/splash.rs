use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use ratatui_braille_bar::BrailleSpinner;
use rust_i18n::t;

use crate::tui::{anim, spinner, theme::Theme, widgets::chrome};

const TAGLINE: &str = "Broadcast control, without breaking flow.";
const APP_DESCRIPTION: &str =
    "Scenes, audio, profiles, recording, and live telemetry — one command center.";
const PROGRESS_BAR_WIDTH: usize = 30;

/// Width of the shimmering "preparing" noise band under the boot label.
const PREPARING_BAND_WIDTH: usize = 44;
const LARGE_LOGO: &[&str] = &[
    " ██████╗ ██████╗ ███████╗ ██████╗████████╗██╗     ",
    "██╔═══██╗██╔══██╗██╔════╝██╔════╝╚══██╔══╝██║     ",
    "██║   ██║██████╔╝███████╗██║        ██║   ██║     ",
    "██║   ██║██╔══██╗╚════██║██║        ██║   ██║     ",
    "╚██████╔╝██████╔╝███████║╚██████╗   ██║   ███████╗",
    " ╚═════╝ ╚═════╝ ╚══════╝ ╚═════╝   ╚═╝   ╚══════╝",
];

/// Centre a card of `width` x `height` cells inside `area`, clamped to what
/// `area` actually has.
///
/// All three splash variants draw a single card floating in the middle of an
/// otherwise empty frame, so each one used to spell out the same pair of
/// splits: a vertical one with the card's height between two elastic gaps,
/// then a horizontal one with its width between two more. Written three times
/// it was three chances to centre a card slightly differently.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let rows = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(height.min(area.height)),
        Constraint::Min(0),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(width.min(area.width)),
        Constraint::Min(0),
    ])
    .split(rows[1])[1]
}

/// Animated startup identity with a responsive large wordmark, moving color
/// wave, orbiting symbols, and a segmented multi-color boot indicator.
///
/// `advanced_ui` picks between the Unicode splash and the ASCII one;
/// `show_icons` only chooses glyphs inside the Unicode one, matching how
/// `TuiModel` gates the rest of the interface.
pub fn render_with_appearance(
    f: &mut Frame,
    theme: Theme,
    frame: u64,
    total_frames: u64,
    show_icons: bool,
    advanced_ui: bool,
) {
    if !advanced_ui {
        render_ascii(f, theme, frame, total_frames);
        return;
    }

    let area = f.area();
    let large = area.width >= 76 && area.height >= 16;
    if large {
        render_large(f, theme, frame, total_frames, show_icons);
        return;
    }

    let card = centered(area, 60, 13);

    let pulse = ((frame as f32 * 0.18).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    let border = anim::blend(theme.accent, theme.accent_alt, pulse);
    let orbit = spinner::splash_frame(show_icons, frame);
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
    let Some(inner) = chrome::frame(f, card, block) else {
        return;
    };

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
    if inner.height >= 9 {
        lines.extend(preparing_lines(
            theme,
            frame,
            (inner.width as usize).min(PREPARING_BAND_WIDTH),
            show_icons,
        ));
    }

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    lines.push(progress_line(progress, theme, frame, true).alignment(Alignment::Center));
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
    let area = centered(f.area(), 64, 15);
    let card = Block::default()
        .borders(Borders::ALL)
        .border_set(chrome::ASCII_BORDER)
        .border_style(Style::default().fg(theme.accent))
        .title(" OBSCTL STARTUP ");
    let Some(inner) = chrome::frame(f, area, card) else {
        return;
    };

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    let tick = spinner::splash_frame(false, frame);
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
            format!("{tick}  (o) LIVE  {tick}"),
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center),
    ]);
    lines.extend(preparing_lines(
        theme,
        frame,
        (inner.width as usize).min(PREPARING_BAND_WIDTH),
        false,
    ));
    lines.extend([
        progress_line(progress, theme, frame, false).alignment(Alignment::Center),
        Line::styled(
            format!("{tick} initializing studio link"),
            Style::default().fg(theme.muted),
        )
        .alignment(Alignment::Center),
    ]);
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_large(f: &mut Frame, theme: Theme, frame: u64, total_frames: u64, show_icons: bool) {
    let stage = centered(f.area(), 88, 16);
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
    let Some(inner) = chrome::frame(f, sections[2], card) else {
        return;
    };

    let progress = (frame as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0);
    let mut lines = vec![
        loader_line("SIGNAL", slither(frame, 32), theme.accent, theme),
        loader_line("LIQUID", liquid_wave(frame, 32), theme.accent_alt, theme),
    ];
    lines.extend(preparing_lines(
        theme,
        frame,
        (inner.width as usize).min(PREPARING_BAND_WIDTH),
        show_icons,
    ));
    lines.extend([
        progress_line(progress, theme, frame, true).alignment(Alignment::Center),
        Line::styled(
            boot_message(progress, show_icons),
            Style::default().fg(theme.muted),
        )
        .alignment(Alignment::Center),
    ]);
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

/// The "Preparing…" boot band: a labelled spinner over a shimmering field of
/// braille dots.
///
/// The band is deliberately non-deterministic — `BrailleSpinner` reseeds from
/// the RNG on every render, which is what produces the shimmer. That makes it
/// the one animation here that is not a pure function of `frame`, so tests
/// assert its shape (width, label) rather than its contents.
///
/// Terminals without Unicode get [`ascii_noise`], a frame-seeded stand-in that
/// shimmers the same way while staying inside ASCII.
fn preparing_lines(theme: Theme, frame: u64, width: usize, rich: bool) -> Vec<Line<'static>> {
    let tick = spinner::splash_frame(rich, frame);
    let label = Line::from(vec![
        Span::styled(
            format!("{tick} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            t!("tui.splash.preparing").into_owned(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ])
    .alignment(Alignment::Center);

    let band = if rich {
        BrailleSpinner::new()
            .color(theme.accent)
            .into_line(width)
            .alignment(Alignment::Center)
    } else {
        Line::styled(ascii_noise(frame, width), Style::default().fg(theme.accent))
            .alignment(Alignment::Center)
    };

    vec![label, band]
}

/// Frame-seeded ASCII shimmer. A tiny xorshift keeps it reproducible for a
/// given (frame, column) so the ASCII splash stays testable, while still
/// looking like noise as the frame advances.
fn ascii_noise(frame: u64, width: usize) -> String {
    const GLYPHS: &[u8] = b" .:-=+*#%@";
    (0..width)
        .map(|column| {
            let mut state = frame
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(column as u64 + 1);
            state ^= state >> 33;
            state = state.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
            state ^= state >> 29;
            GLYPHS[(state % GLYPHS.len() as u64) as usize] as char
        })
        .collect()
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

/// The boot progress bar, in whichever alphabet the splash is drawing in.
///
/// Both splashes show a bar of exactly [`PROGRESS_BAR_WIDTH`] cells; only the
/// glyphs and the coloring differ, so both live here and the width is written
/// down once. The Unicode bar tints every filled cell along the accent
/// gradient and rides a blinking cursor glyph on its leading edge, which needs
/// one span per cell. The ASCII bar has no per-cell color to express, so it is
/// a single bracketed string.
fn progress_line(progress: f32, theme: Theme, frame: u64, rich: bool) -> Line<'static> {
    let filled = (progress * PROGRESS_BAR_WIDTH as f32).round() as usize;
    let percent = (progress * 100.0).round() as u8;

    if !rich {
        return Line::styled(
            format!(
                "[{}{}] {percent:>3}%",
                "#".repeat(filled),
                ".".repeat(PROGRESS_BAR_WIDTH.saturating_sub(filled))
            ),
            Style::default().fg(theme.info),
        );
    }

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
        format!("  {percent:>3}%"),
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
