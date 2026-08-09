//! Broadcast-state spinners and the oversized block font used by the
//! top-right status pane.
//!
//! Frames come from the `rattles` preset library. `rattles` can drive itself
//! off the wall clock (`Rattler::current_frame`), but the TUI advances every
//! animation from `anim::AnimClock` ticks instead — see that module for why —
//! so this module only ever uses the index-based `Rattler::frame`, which keeps
//! rendering a pure function of the tick counter and therefore testable.
//!
//! Each state gets a visually distinct preset so the broadcast state is
//! readable from the spinner alone, without reading the label:
//!
//! | State | Rich (Unicode)                | ASCII fallback              |
//! |-------|-------------------------------|-----------------------------|
//! | Idle  | `orbit` — slow braille circle | `simple_dots_scrolling`     |
//! | Live  | `circle_halves` — rotation    | `rolling_line` (`/ - \ |`)  |
//! | Rec   | `pulse` — braille throb       | `balloon` (`. o O o .`)     |
//!
//! `breathe` is the more obvious fit for "idle", but it fades to a blank
//! frame twice per cycle, which reads as a rendering gap next to a static
//! label. `orbit` stays continuously visible.

use rattles::presets::prelude as presets;

/// What the broadcast is doing right now.
///
/// `Live` and `Rec` are not exclusive — OBS streams and records
/// independently, so the status pane can show both at once. `Idle` is what
/// remains when neither is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BroadcastState {
    Idle,
    Live,
    Rec,
}

impl BroadcastState {
    /// The word rendered in the block font. Not localized on purpose: these
    /// are drawn as glyph art from a fixed A-Z subset, the same way the
    /// splash wordmark is.
    pub fn label(self) -> &'static str {
        match self {
            BroadcastState::Idle => "IDLE",
            BroadcastState::Live => "LIVE",
            BroadcastState::Rec => "REC",
        }
    }

    /// How many clock ticks each spinner frame is held for. Idle breathes
    /// slowly; live spins fastest so an active stream reads as urgent.
    fn ticks_per_frame(self) -> u64 {
        match self {
            BroadcastState::Idle => 3,
            BroadcastState::Live => 2,
            BroadcastState::Rec => 3,
        }
    }
}

/// Which states to display for a stream/record pair. Empty is impossible —
/// "nothing is happening" is itself a state.
pub fn active_states(streaming: bool, recording: bool) -> Vec<BroadcastState> {
    let mut states = Vec::with_capacity(2);
    if streaming {
        states.push(BroadcastState::Live);
    }
    if recording {
        states.push(BroadcastState::Rec);
    }
    if states.is_empty() {
        states.push(BroadcastState::Idle);
    }
    states
}

/// The spinner frame for `state` at animation tick `tick`.
///
/// Frame width is constant within a state (every frame of a given preset has
/// the same width), so a badge never reflows as it animates.
pub fn frame(state: BroadcastState, rich: bool, tick: u64) -> &'static str {
    let index = (tick / state.ticks_per_frame()) as usize;
    match (state, rich) {
        (BroadcastState::Idle, true) => presets::orbit().frame(index),
        (BroadcastState::Idle, false) => presets::simple_dots_scrolling().frame(index),
        (BroadcastState::Live, true) => presets::circle_halves().frame(index),
        (BroadcastState::Live, false) => presets::rolling_line().frame(index),
        (BroadcastState::Rec, true) => presets::pulse().frame(index),
        (BroadcastState::Rec, false) => presets::balloon().frame(index),
    }
}

/// Splash-screen spinner — a different preset again, so the boot animation is
/// not mistaken for a broadcast state.
pub fn splash_frame(rich: bool, tick: u64) -> &'static str {
    let index = (tick / 2) as usize;
    if rich {
        presets::dots().frame(index)
    } else {
        presets::rolling_line().frame(index)
    }
}

/// Rows in the block font. Fixed at 3 so callers can reserve height up front.
pub const BLOCK_HEIGHT: usize = 3;

/// Cell columns per glyph, plus one column of spacing between glyphs.
const GLYPH_WIDTH: usize = 3;

/// A 3x3 block font covering only the letters the state labels need. Anything
/// outside this set renders as blank cells rather than panicking, so adding a
/// state without extending the font degrades instead of crashing.
fn glyph(letter: char) -> [&'static str; BLOCK_HEIGHT] {
    match letter {
        'I' => ["###", " # ", "###"],
        'D' => ["## ", "# #", "## "],
        'L' => ["#  ", "#  ", "###"],
        'E' => ["###", "## ", "###"],
        'V' => ["# #", "# #", " # "],
        'R' => ["## ", "## ", "# #"],
        'C' => ["###", "#  ", "###"],
        _ => ["   ", "   ", "   "],
    }
}

/// Rendered width of `word` in the block font, in terminal columns.
pub fn block_width(word: &str) -> usize {
    let letters = word.chars().count();
    if letters == 0 {
        0
    } else {
        letters * GLYPH_WIDTH + (letters - 1)
    }
}

/// Render `word` as `BLOCK_HEIGHT` rows of oversized block text.
///
/// `fill` is the glyph painted for "on" cells — `█` normally, `#` in ASCII
/// mode — so the same font serves both appearances.
pub fn block_word(word: &str, fill: char) -> [String; BLOCK_HEIGHT] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (position, letter) in word.chars().enumerate() {
        let cells = glyph(letter.to_ascii_uppercase());
        for (row, target) in rows.iter_mut().enumerate() {
            if position > 0 {
                target.push(' ');
            }
            target.push_str(&cells[row].replace('#', &fill.to_string()));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_the_fallback_when_nothing_is_running() {
        assert_eq!(active_states(false, false), vec![BroadcastState::Idle]);
    }

    #[test]
    fn streaming_and_recording_are_reported_together() {
        assert_eq!(active_states(true, false), vec![BroadcastState::Live]);
        assert_eq!(active_states(false, true), vec![BroadcastState::Rec]);
        assert_eq!(
            active_states(true, true),
            vec![BroadcastState::Live, BroadcastState::Rec]
        );
    }

    #[test]
    fn each_state_uses_a_distinguishable_spinner() {
        // The point of the feature: at any tick, no two states share a frame,
        // so the spinner alone identifies the state.
        for rich in [true, false] {
            for tick in 0..64 {
                let idle = frame(BroadcastState::Idle, rich, tick);
                let live = frame(BroadcastState::Live, rich, tick);
                let rec = frame(BroadcastState::Rec, rich, tick);
                assert_ne!(idle, live, "idle/live collide at tick {tick} (rich={rich})");
                assert_ne!(idle, rec, "idle/rec collide at tick {tick} (rich={rich})");
                assert_ne!(live, rec, "live/rec collide at tick {tick} (rich={rich})");
            }
        }
    }

    #[test]
    fn spinner_frames_keep_a_stable_width_within_a_state() {
        for state in [
            BroadcastState::Idle,
            BroadcastState::Live,
            BroadcastState::Rec,
        ] {
            for rich in [true, false] {
                let width = frame(state, rich, 0).chars().count();
                for tick in 1..64 {
                    assert_eq!(
                        frame(state, rich, tick).chars().count(),
                        width,
                        "{state:?} changed width at tick {tick} (rich={rich})"
                    );
                }
            }
        }
    }

    #[test]
    fn no_rich_spinner_frame_is_blank() {
        // A frame of pure whitespace (braille U+2800 included) reads as a
        // rendering gap next to a static label rather than as an animation.
        //
        // Only the rich presets are held to this. The ASCII idle fallback is
        // a scrolling ellipsis (`.` → `..` → `...` → blank), where the empty
        // frame is the reset everyone already recognises from "loading...".
        for state in [
            BroadcastState::Idle,
            BroadcastState::Live,
            BroadcastState::Rec,
        ] {
            for tick in 0..64 {
                let text = frame(state, true, tick);
                assert!(
                    text.chars().any(|c| !c.is_whitespace() && c != '\u{2800}'),
                    "{state:?} is blank at tick {tick}"
                );
            }
        }
    }

    #[test]
    fn spinner_advances_over_time_and_cycles() {
        let first = frame(BroadcastState::Live, true, 0);
        let advanced = (0..32).any(|tick| frame(BroadcastState::Live, true, tick) != first);
        assert!(advanced, "live spinner never changed frame");
    }

    #[test]
    fn splash_spinner_differs_between_rich_and_ascii() {
        assert_ne!(splash_frame(true, 0), splash_frame(false, 0));
    }

    #[test]
    fn block_word_has_three_rows_of_equal_declared_width() {
        let rows = block_word("REC", '█');
        assert_eq!(rows.len(), BLOCK_HEIGHT);
        for row in &rows {
            assert_eq!(row.chars().count(), block_width("REC"));
        }
        assert_eq!(block_width("REC"), 11);
        assert_eq!(block_width("IDLE"), 15);
        assert_eq!(block_width("LIVE"), 15);
    }

    #[test]
    fn block_word_uses_the_requested_fill_glyph() {
        assert!(block_word("LIVE", '#').iter().any(|row| row.contains('#')));
        assert!(block_word("LIVE", '█').iter().any(|row| row.contains('█')));
        assert!(!block_word("LIVE", '█').iter().any(|row| row.contains('#')));
    }

    #[test]
    fn block_word_is_blank_for_empty_input() {
        assert_eq!(block_width(""), 0);
        assert_eq!(block_word("", '█'), ["", "", ""].map(String::from));
    }

    #[test]
    fn unknown_letters_render_blank_instead_of_panicking() {
        let rows = block_word("Z", '█');
        assert_eq!(rows[0], "   ");
    }

    #[test]
    fn every_state_label_is_covered_by_the_font() {
        for state in [
            BroadcastState::Idle,
            BroadcastState::Live,
            BroadcastState::Rec,
        ] {
            let rows = block_word(state.label(), '█');
            assert!(
                rows.iter().any(|row| row.contains('█')),
                "{state:?} label {:?} renders blank — font is missing a letter",
                state.label()
            );
        }
    }
}
