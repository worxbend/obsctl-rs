//! A small tick-based animation clock. `AnimClock::tick` is advanced once
//! per render tick (see `tui::app::run_loop`'s ticker arm) so animation speed
//! tracks the configured refresh interval rather than raw wall-clock time —
//! this keeps redraw-triggered draws (IPC events) from skewing pulse speed.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AnimClock {
    pub frame: u64,
}

impl AnimClock {
    pub fn tick(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// A smooth 0..1 pulse with the given period (in ticks).
    pub fn pulse(&self, period_ticks: u64) -> f32 {
        let period_ticks = period_ticks.max(1);
        let phase = (self.frame % period_ticks) as f32 / period_ticks as f32;
        (phase * std::f32::consts::TAU).sin() * 0.5 + 0.5
    }

    /// Cycle through `frames` at one step per `ticks_per_frame` clock ticks.
    pub fn spinner<'a>(&self, frames: &'a [&'a str], ticks_per_frame: u64) -> &'a str {
        if frames.is_empty() {
            return "";
        }
        let ticks_per_frame = ticks_per_frame.max(1);
        let idx = ((self.frame / ticks_per_frame) as usize) % frames.len();
        frames[idx]
    }
}

/// Linearly blend two truecolor `Color`s; falls back to `a` for non-Rgb
/// variants (e.g. the `mono` theme's named colors) since blending named
/// terminal colors isn't meaningful.
pub fn blend(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let lerp =
                |x: u8, y: u8| -> u8 { (x as f32 + (y as f32 - x as f32) * t).round() as u8 };
            Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
        }
        _ => a,
    }
}

/// Build an animated per-character gradient. The wave makes headings feel
/// alive without changing layout or relying on terminal-specific effects.
pub fn gradient_line(text: &str, from: Color, to: Color, frame: u64, bold: bool) -> Line<'static> {
    let width = text.chars().count().max(1) as f32;
    let spans = text
        .chars()
        .enumerate()
        .map(|(index, ch)| {
            let position = index as f32 / width;
            let wave = ((position * std::f32::consts::TAU) + frame as f32 * 0.12).sin();
            let color = blend(from, to, wave * 0.5 + 0.5);
            let mut style = Style::default().fg(color);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            Span::styled(ch.to_string(), style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// Render numeric history as a compact Unicode sparkline. Empty history is
/// represented by a dim baseline so the dashboard does not jump as data arrives.
pub fn sparkline(values: &[f64], width: usize) -> String {
    const BARS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if width == 0 {
        return String::new();
    }
    if values.is_empty() {
        return "▁".repeat(width);
    }

    let start = values.len().saturating_sub(width);
    let visible = &values[start..];
    let max = visible.iter().copied().fold(0.0_f64, f64::max).max(1.0);
    let mut result = "▁".repeat(width.saturating_sub(visible.len()));
    for value in visible {
        let normalized = (value / max).clamp(0.0, 1.0);
        let index = (normalized * (BARS.len() - 1) as f64).round() as usize;
        result.push(BARS[index]);
    }
    result
}

/// ASCII-only history graph for terminals without Unicode block support.
pub fn sparkline_ascii(values: &[f64], width: usize) -> String {
    const LEVELS: &[u8] = b".-=+*#";
    if width == 0 {
        return String::new();
    }
    if values.is_empty() {
        return ".".repeat(width);
    }
    let visible = &values[values.len().saturating_sub(width)..];
    let min = visible.iter().copied().fold(f64::INFINITY, f64::min);
    let max = visible.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = (max - min).max(f64::EPSILON);
    let mut result = ".".repeat(width.saturating_sub(visible.len()));
    for value in visible {
        let index = (((value - min) / range) * (LEVELS.len() - 1) as f64).round() as usize;
        result.push(LEVELS[index.min(LEVELS.len() - 1)] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_increments_frame() {
        let mut clock = AnimClock::default();
        assert_eq!(clock.frame, 0);
        clock.tick();
        clock.tick();
        assert_eq!(clock.frame, 2);
    }

    #[test]
    fn pulse_is_bounded_between_zero_and_one() {
        let mut clock = AnimClock::default();
        for _ in 0..40 {
            let p = clock.pulse(10);
            assert!((0.0..=1.0).contains(&p));
            clock.tick();
        }
    }

    #[test]
    fn pulse_returns_to_start_after_full_period() {
        let mut clock = AnimClock::default();
        let start = clock.pulse(8);
        for _ in 0..8 {
            clock.tick();
        }
        assert!((clock.pulse(8) - start).abs() < 1e-6);
    }

    #[test]
    fn spinner_cycles_through_frames() {
        let frames = ["a", "b", "c"];
        let mut clock = AnimClock::default();
        assert_eq!(clock.spinner(&frames, 1), "a");
        clock.tick();
        assert_eq!(clock.spinner(&frames, 1), "b");
        clock.tick();
        assert_eq!(clock.spinner(&frames, 1), "c");
        clock.tick();
        assert_eq!(clock.spinner(&frames, 1), "a");
    }

    #[test]
    fn spinner_handles_empty_frames() {
        let clock = AnimClock::default();
        assert_eq!(clock.spinner(&[], 1), "");
    }

    #[test]
    fn blend_interpolates_rgb_colors() {
        let a = Color::Rgb(0, 0, 0);
        let b = Color::Rgb(100, 200, 255);
        assert_eq!(blend(a, b, 0.0), a);
        assert_eq!(blend(a, b, 1.0), b);
        assert_eq!(blend(a, b, 0.5), Color::Rgb(50, 100, 128));
    }

    #[test]
    fn blend_falls_back_to_first_color_for_non_rgb() {
        assert_eq!(blend(Color::Red, Color::Blue, 0.5), Color::Red);
    }

    #[test]
    fn gradient_line_preserves_text() {
        let line = gradient_line("OBS", Color::Rgb(0, 0, 0), Color::Rgb(255, 0, 0), 3, true);
        let rendered: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(rendered, "OBS");
    }

    #[test]
    fn sparkline_is_fixed_width_and_tracks_peaks() {
        assert_eq!(sparkline(&[], 4), "▁▁▁▁");
        let line = sparkline(&[0.0, 5.0, 10.0], 4);
        assert_eq!(line.chars().count(), 4);
        assert!(line.ends_with('█'));
    }
}
