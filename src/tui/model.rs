use std::collections::HashMap;

use time::OffsetDateTime;

use crate::{
    ipc::protocol::{LogEvent, LogLevel},
    obs::state::{AudioState, ObsSnapshot, ObsStats, SceneState, ServerStatus},
    tui::{
        anim::AnimClock, input::MAX_COUNT, keymap::Pending, series::RollingSeries, theme::Theme,
    },
};

pub use crate::domain::parser::DEFAULT_PALETTE_PREFIX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPanel {
    #[default]
    Scenes,
    Audio,
    Profiles,
    Collections,
}

impl FocusPanel {
    /// Panels are arranged as a 2x2 grid:
    /// ```text
    /// Scenes    Audio
    /// Profiles  Collections
    /// ```
    /// These map Ctrl+arrow / Ctrl+hjkl navigation across that grid; moving
    /// past an edge is a no-op (stays on the current panel).
    pub fn left(self) -> Self {
        match self {
            Self::Audio => Self::Scenes,
            Self::Collections => Self::Profiles,
            other => other,
        }
    }

    pub fn right(self) -> Self {
        match self {
            Self::Scenes => Self::Audio,
            Self::Profiles => Self::Collections,
            other => other,
        }
    }

    pub fn up(self) -> Self {
        match self {
            Self::Profiles => Self::Scenes,
            Self::Collections => Self::Audio,
            other => other,
        }
    }

    pub fn down(self) -> Self {
        match self {
            Self::Scenes => Self::Profiles,
            Self::Audio => Self::Collections,
            other => other,
        }
    }

    /// Reading order of the 2x2 grid, used by Tab / Shift-Tab. Unlike the
    /// spatial moves above this one wraps, so repeated Tab visits every
    /// panel and comes back around.
    pub const CYCLE: [Self; 4] = [Self::Scenes, Self::Audio, Self::Profiles, Self::Collections];

    fn cycle_index(self) -> usize {
        Self::CYCLE.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::CYCLE[(self.cycle_index() + 1) % Self::CYCLE.len()]
    }

    pub fn prev(self) -> Self {
        Self::CYCLE[(self.cycle_index() + Self::CYCLE.len() - 1) % Self::CYCLE.len()]
    }
}

/// Top-level screen the TUI is showing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Main,
    Settings,
}

/// One audio input's volume-meter state.
///
/// OBS reports a bare magnitude several times a second and nothing else, but
/// its own meter shows two moving things: a bar that falls off gradually
/// after the sound stops (a "peak program meter"), and a marker parked at the
/// loudest value from the last few seconds. Both are derived here from the
/// raw samples so the terminal meter reads the way the OBS one does.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterReading {
    /// Magnitude (0-1) reported by the most recent `InputVolumeMeters` event.
    pub level: f32,
    /// Peak program level (0-1) — `level` with fall-off decay applied. This
    /// is what the meter's main bar draws.
    pub peak_program: f32,
    /// Loudest level (0-1) seen in the last few seconds, drawn as a marker
    /// above the bar so a brief clip stays visible after it has passed.
    pub peak_hold: f32,
}

/// Fraction of the bar that survives one meter sample once the input goes
/// quiet. Multiplying by a constant is a constant drop in decibels, so 0.88
/// is roughly -1.1 dB per sample; at the ~20 samples a second obs-websocket
/// sends that lands near OBS's own 23.5 dB/s default fall-off.
const PEAK_PROGRAM_FALLOFF: f32 = 0.88;
/// The same idea for the peak marker, tuned so a peak takes ~20 seconds to
/// slide from the top of the meter to its floor — OBS's peak hold window.
const PEAK_HOLD_FALLOFF: f32 = 0.985;

/// The four cumulative frame counters `GetStats` reports.
///
/// OBS counts these since *it* launched, not since the stream started, so
/// the stats pane subtracts a per-stream baseline (see
/// [`TuiModel::stream_frame_baseline`]) to show what the current broadcast
/// actually dropped — a stream that starts on an hours-old OBS session
/// would otherwise inherit every frame that machine has ever missed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameCounters {
    pub render_skipped: u64,
    pub render_total: u64,
    pub output_skipped: u64,
    pub output_total: u64,
}

impl FrameCounters {
    pub fn from_stats(stats: &ObsStats) -> Self {
        Self {
            render_skipped: stats.render_skipped_frames,
            render_total: stats.render_total_frames,
            output_skipped: stats.output_skipped_frames,
            output_total: stats.output_total_frames,
        }
    }

    /// Counters accumulated since `baseline`. Saturating, so an OBS restart
    /// mid-stream (which resets the counters) reads as zero drops rather
    /// than wrapping to a huge number.
    pub fn since(self, baseline: Self) -> Self {
        Self {
            render_skipped: self.render_skipped.saturating_sub(baseline.render_skipped),
            render_total: self.render_total.saturating_sub(baseline.render_total),
            output_skipped: self.output_skipped.saturating_sub(baseline.output_skipped),
            output_total: self.output_total.saturating_sub(baseline.output_total),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    pub snapshot: Option<ObsSnapshot>,
    pub server_status: Option<ServerStatus>,
    pub logs: Vec<TuiLogEntry>,
    pub command_palette: CommandPaletteState,
    pub last_result: Option<String>,
    /// Tick at which `last_result` was last set; drives the typewriter
    /// reveal animation in the command palette.
    pub last_result_tick: u64,
    pub connected_to_daemon: bool,
    pub focus: FocusPanel,
    pub scene_cursor: usize,
    pub audio_cursor: usize,
    pub profile_cursor: usize,
    pub collection_cursor: usize,
    /// Meter state per input name, folded from `InputVolumeMeters` events by
    /// [`record_meter_level`](TuiModel::record_meter_level).
    pub meters: HashMap<String, MeterReading>,
    /// Active color theme, chosen via config or the settings view.
    pub theme: Theme,
    /// Whether rich Unicode icons/emoji are enabled. When false, widgets use
    /// compact ASCII fallbacks for restricted terminals.
    pub show_icons: bool,
    /// Whether rich borders, Unicode charts, gradients, and terminal art are
    /// enabled. False selects the simplified ASCII-safe rendering path.
    pub advanced_ui: bool,
    /// Short rolling histories used by the animated status sparklines and by
    /// the braille meters, which need a peak to scale and mark against.
    pub cpu_history: RollingSeries,
    pub memory_history: RollingSeries,
    pub bitrate_history: RollingSeries,
    pub fps_history: RollingSeries,
    pub frame_time_history: RollingSeries,
    /// Lifetime frame counters as of the moment the current stream started,
    /// used to report per-stream drops in the stats pane. `None` while not
    /// streaming; set on the first stats sample after streaming begins.
    pub stream_frame_baseline: Option<FrameCounters>,
    /// Advances once per render tick; drives pulsing/spinner animations.
    pub anim: AnimClock,
    /// (scene name, tick it became active) — drives the brief flash
    /// highlight in the scenes panel right after a switch. Set by the
    /// event applier when it observes `current_scene` change.
    pub scene_flash: Option<(String, u64)>,
    /// Current top-level screen (main dashboard or the settings/theme picker).
    pub view: View,
    /// Cursor into `theme::ALL` while the settings view is open.
    pub settings_cursor: usize,
    /// Theme active before opening the settings view, restored on Esc
    /// without confirming a new choice (live-preview-then-cancel, btop-style).
    pub theme_preview_origin: Option<Theme>,
    /// Half-typed vim key sequence (`g…`, `<leader>…`). Anything other than
    /// [`Pending::None`] puts the which-key overlay on screen and routes the
    /// next keypress through [`crate::tui::keymap::resolve`].
    pub pending: Pending,
    /// Numeric count prefix accumulated so far (`12j`), if any.
    pub pending_count: Option<u32>,
    /// Character the command line opens with — `ui.command_palette_prefix`.
    pub palette_prefix: char,
    /// Lines the log pane is scrolled back from the live tail. `0` follows.
    pub log_scroll: usize,
    /// Cached visible (non-hidden) scenes; rebuilt in `clamp_cursors` after each snapshot update.
    cached_visible_scenes: Vec<SceneState>,
}

impl Default for TuiModel {
    fn default() -> Self {
        Self {
            snapshot: None,
            server_status: None,
            logs: Vec::new(),
            command_palette: CommandPaletteState::default(),
            last_result: None,
            last_result_tick: 0,
            connected_to_daemon: false,
            focus: FocusPanel::default(),
            scene_cursor: 0,
            audio_cursor: 0,
            profile_cursor: 0,
            collection_cursor: 0,
            meters: HashMap::new(),
            theme: Theme::default_theme(),
            show_icons: true,
            advanced_ui: true,
            cpu_history: RollingSeries::default(),
            memory_history: RollingSeries::default(),
            bitrate_history: RollingSeries::default(),
            fps_history: RollingSeries::default(),
            frame_time_history: RollingSeries::default(),
            stream_frame_baseline: None,
            anim: AnimClock::default(),
            scene_flash: None,
            view: View::default(),
            settings_cursor: 0,
            theme_preview_origin: None,
            pending: Pending::default(),
            pending_count: None,
            palette_prefix: DEFAULT_PALETTE_PREFIX,
            log_scroll: 0,
            cached_visible_scenes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiLogEntry {
    pub level: LogLevel,
    pub message: String,
    pub target: Option<String>,
    pub timestamp: OffsetDateTime,
}

#[derive(Debug, Clone, Default)]
pub struct CommandPaletteState {
    pub active: bool,
    pub input: String,
    pub completions: Vec<String>,
    pub completion_idx: Option<usize>,
}

impl CommandPaletteState {
    /// Ctrl-U — wipe the line back to its prompt prefix.
    pub fn clear_to_prefix(&mut self) {
        self.input = self.input.chars().take(1).collect();
    }

    /// Ctrl-W — delete the word before the cursor, never eating the prefix.
    pub fn delete_word(&mut self) {
        let floor = self.input.chars().next().map(char::len_utf8).unwrap_or(0);
        let mut end = self.input.len();
        while end > floor && self.input[..end].ends_with(' ') {
            end -= 1;
        }
        while end > floor && !self.input[..end].ends_with(' ') {
            end -= 1;
            while end > floor && !self.input.is_char_boundary(end) {
                end -= 1;
            }
        }
        self.input.truncate(end);
    }

    pub fn cycle_next(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let next = match self.completion_idx {
            None => 0,
            Some(i) => (i + 1) % self.completions.len(),
        };
        self.completion_idx = Some(next);
        self.input = self.completions[next].clone();
    }

    pub fn cycle_prev(&mut self) {
        if self.completions.is_empty() {
            return;
        }
        let prev = match self.completion_idx {
            None => self.completions.len() - 1,
            Some(0) => self.completions.len() - 1,
            Some(i) => i - 1,
        };
        self.completion_idx = Some(prev);
        self.input = self.completions[prev].clone();
    }
}

impl From<LogEvent> for TuiLogEntry {
    fn from(event: LogEvent) -> Self {
        Self {
            level: event.level,
            message: event.message,
            target: event.target,
            timestamp: event.timestamp,
        }
    }
}

impl TuiModel {
    pub const MAX_LOG_ENTRIES: usize = 200;

    /// Build a model with the given starting theme (used at startup, once
    /// the configured/custom theme has been resolved).
    pub fn with_theme(theme: Theme) -> Self {
        Self {
            theme,
            ..Self::default()
        }
    }

    pub fn with_appearance(theme: Theme, show_icons: bool, advanced_ui: bool) -> Self {
        Self {
            theme,
            show_icons,
            advanced_ui,
            ..Self::default()
        }
    }

    /// The typed count prefix, or 1 when none was typed — the multiplier
    /// every motion applies (`12j` moves twelve rows).
    pub fn count(&self) -> usize {
        self.pending_count.unwrap_or(1).max(1) as usize
    }

    /// Append a digit to the count prefix, saturating at [`MAX_COUNT`].
    pub fn push_count(&mut self, digit: u32) {
        let next = self
            .pending_count
            .unwrap_or(0)
            .saturating_mul(10)
            .saturating_add(digit);
        self.pending_count = Some(next.min(MAX_COUNT));
    }

    /// Abandon any half-typed sequence: closes the which-key overlay and
    /// drops the count prefix.
    pub fn clear_pending(&mut self) {
        self.pending = Pending::None;
        self.pending_count = None;
    }

    pub fn symbol(&self, rich: &'static str, ascii: &'static str) -> &'static str {
        if self.rich_ui() { rich } else { ascii }
    }

    pub fn rich_ui(&self) -> bool {
        self.show_icons && self.advanced_ui
    }

    pub fn record_metric_sample(&mut self) {
        let stats = self.stats().copied();
        if let Some(stats) = stats {
            self.cpu_history.push(stats.cpu_usage_percent);
            self.memory_history.push(stats.memory_usage_mb);
            self.fps_history.push(stats.active_fps);
            self.frame_time_history
                .push(stats.average_frame_render_time_ms);
        }
        if let Some(bitrate) = self.stream_bitrate_kbps() {
            self.bitrate_history.push(bitrate);
        }
        // Latch the per-stream frame baseline on the first sample of a
        // stream and drop it when the stream ends, so drops are always
        // reported relative to the broadcast currently on air.
        match (self.streaming(), stats) {
            (true, Some(stats)) => {
                self.stream_frame_baseline
                    .get_or_insert_with(|| FrameCounters::from_stats(&stats));
            }
            (false, _) => self.stream_frame_baseline = None,
            (true, None) => {}
        }
    }

    /// Set `last_result` and mark the current tick as its reveal start, so
    /// the command palette can replay it with a typewriter animation.
    pub fn set_last_result(&mut self, message: impl Into<String>) {
        self.last_result = Some(message.into());
        self.last_result_tick = self.anim.frame;
    }

    /// Characters of `last_result` revealed so far, per `chars_per_tick`.
    /// Returns `None` if there is no result to show.
    pub fn revealed_last_result(&self, chars_per_tick: usize) -> Option<&str> {
        let text = self.last_result.as_deref()?;
        let elapsed = self.anim.frame.saturating_sub(self.last_result_tick) as usize;
        let revealed_chars = elapsed.saturating_mul(chars_per_tick.max(1));
        match text.char_indices().nth(revealed_chars) {
            Some((byte_idx, _)) => Some(&text[..byte_idx]),
            None => Some(text),
        }
    }

    pub fn push_log(&mut self, entry: TuiLogEntry) {
        self.logs.push(entry);
        if self.logs.len() > Self::MAX_LOG_ENTRIES {
            let overflow = self.logs.len() - Self::MAX_LOG_ENTRIES;
            self.logs.drain(0..overflow);
        }
        // While the user is reading scrolled-back history, hold the viewport
        // on the same lines rather than letting new arrivals push them away.
        if self.log_scroll > 0 {
            self.log_scroll = (self.log_scroll + 1).min(Self::MAX_LOG_ENTRIES);
        }
    }

    /// Scroll the log pane back from the live tail by `lines`. `visible` is
    /// the pane's current row count, which bounds how far back there is to go.
    pub fn scroll_logs_up(&mut self, lines: usize, visible: usize) {
        let max = self.logs.len().saturating_sub(visible);
        self.log_scroll = self.log_scroll.saturating_add(lines).min(max);
    }

    /// Scroll back toward the live tail; reaching `0` resumes following.
    pub fn scroll_logs_down(&mut self, lines: usize) {
        self.log_scroll = self.log_scroll.saturating_sub(lines);
    }

    /// Index of the first log entry to render given a pane showing `visible`
    /// rows, honouring how far the user has scrolled back.
    pub fn log_view_start(&self, visible: usize) -> usize {
        let max = self.logs.len().saturating_sub(visible);
        max - self.log_scroll.min(max)
    }

    /// Visible (non-hidden) scenes, in snapshot order. Returns the cached slice; no allocation per call.
    pub fn scenes(&self) -> &[SceneState] {
        &self.cached_visible_scenes
    }

    pub fn audio_inputs(&self) -> &[AudioState] {
        self.snapshot
            .as_ref()
            .map(|s| s.audio_inputs.as_slice())
            .unwrap_or(&[])
    }

    pub fn profiles(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .map(|s| s.profiles.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_profile(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.current_profile.as_deref())
    }

    pub fn scene_collections(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .map(|s| s.scene_collections.as_slice())
            .unwrap_or(&[])
    }

    pub fn current_scene_collection(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.current_scene_collection.as_deref())
    }

    pub fn stats(&self) -> Option<&ObsStats> {
        self.snapshot.as_ref().and_then(|s| s.stats.as_ref())
    }

    pub fn streaming(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.streaming)
    }

    pub fn recording(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.recording)
    }

    /// Frames rendered and dropped since the current stream started.
    ///
    /// `None` when not streaming, and also on the sample that establishes
    /// the baseline — at that point no frames have been counted *since* it,
    /// so the deltas are all zero and a caller would be dividing by a zero
    /// total. Callers fall back to OBS's lifetime counters until the next
    /// poll gives this something to measure.
    pub fn stream_frame_drops(&self) -> Option<FrameCounters> {
        let stats = self.stats()?;
        let baseline = self.stream_frame_baseline?;
        let drops = FrameCounters::from_stats(stats).since(baseline);
        (drops.render_total > 0 || drops.output_total > 0).then_some(drops)
    }

    pub fn stream_bitrate_kbps(&self) -> Option<f64> {
        self.snapshot.as_ref().and_then(|s| s.stream_bitrate_kbps)
    }

    pub fn stream_duration_ms(&self) -> Option<u64> {
        self.snapshot.as_ref().and_then(|s| s.stream_duration_ms)
    }

    pub fn record_duration_ms(&self) -> Option<u64> {
        self.snapshot.as_ref().and_then(|s| s.record_duration_ms)
    }

    pub fn current_scene(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.current_scene.as_deref())
    }

    pub fn obs_connected(&self) -> bool {
        self.snapshot.as_ref().map(|s| s.connected).unwrap_or(false)
    }

    /// Number of rows in `panel`'s list.
    pub fn panel_len(&self, panel: FocusPanel) -> usize {
        match panel {
            FocusPanel::Scenes => self.scenes().len(),
            FocusPanel::Audio => self.audio_inputs().len(),
            FocusPanel::Profiles => self.profiles().len(),
            FocusPanel::Collections => self.scene_collections().len(),
        }
    }

    /// Cursor position within `panel`'s list.
    pub fn panel_cursor(&self, panel: FocusPanel) -> usize {
        match panel {
            FocusPanel::Scenes => self.scene_cursor,
            FocusPanel::Audio => self.audio_cursor,
            FocusPanel::Profiles => self.profile_cursor,
            FocusPanel::Collections => self.collection_cursor,
        }
    }

    /// Move `panel`'s cursor to `index`, clamped to the list.
    pub fn set_panel_cursor(&mut self, panel: FocusPanel, index: usize) {
        let max = self.panel_len(panel).saturating_sub(1);
        let index = index.min(max);
        match panel {
            FocusPanel::Scenes => self.scene_cursor = index,
            FocusPanel::Audio => self.audio_cursor = index,
            FocusPanel::Profiles => self.profile_cursor = index,
            FocusPanel::Collections => self.collection_cursor = index,
        }
    }

    pub fn move_up(&mut self) {
        self.move_up_by(1);
    }

    pub fn move_down(&mut self) {
        self.move_down_by(1);
    }

    pub fn move_up_by(&mut self, rows: usize) {
        let cursor = self.panel_cursor(self.focus).saturating_sub(rows);
        self.set_panel_cursor(self.focus, cursor);
    }

    pub fn move_down_by(&mut self, rows: usize) {
        let cursor = self.panel_cursor(self.focus).saturating_add(rows);
        self.set_panel_cursor(self.focus, cursor);
    }

    pub fn move_to_top(&mut self) {
        self.set_panel_cursor(self.focus, 0);
    }

    pub fn move_to_bottom(&mut self) {
        self.set_panel_cursor(self.focus, usize::MAX);
    }

    /// Keep cursors within valid list bounds; call after snapshot updates.
    pub fn clamp_cursors(&mut self) {
        self.cached_visible_scenes = self
            .snapshot
            .as_ref()
            .map(|s| s.scenes.iter().filter(|sc| !sc.hidden).cloned().collect())
            .unwrap_or_default();
        let scene_max = self.cached_visible_scenes.len().saturating_sub(1);
        self.scene_cursor = self.scene_cursor.min(scene_max);
        let audio_max = self.audio_inputs().len().saturating_sub(1);
        self.audio_cursor = self.audio_cursor.min(audio_max);
        let profile_max = self.profiles().len().saturating_sub(1);
        self.profile_cursor = self.profile_cursor.min(profile_max);
        let collection_max = self.scene_collections().len().saturating_sub(1);
        self.collection_cursor = self.collection_cursor.min(collection_max);
    }

    pub fn focused_scene(&self) -> Option<&SceneState> {
        self.cached_visible_scenes.get(self.scene_cursor)
    }

    pub fn focused_audio(&self) -> Option<&AudioState> {
        self.audio_inputs().get(self.audio_cursor)
    }

    /// Fold one `InputVolumeMeters` sample into the stored reading for
    /// `name`. The bar and the peak marker only ever jump *up* instantly;
    /// on the way down they decay, which is what stops the meter from
    /// flickering between full and empty on every sample.
    pub fn record_meter_level(&mut self, name: String, level: f32) {
        let level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let reading = self.meters.entry(name).or_default();
        reading.level = level;
        reading.peak_program = level.max(reading.peak_program * PEAK_PROGRAM_FALLOFF);
        reading.peak_hold = level.max(reading.peak_hold * PEAK_HOLD_FALLOFF);
    }

    /// Meter state for `name`, or `None` when OBS has not reported a level
    /// for that input yet.
    pub fn meter(&self, name: &str) -> Option<MeterReading> {
        self.meters.get(name).copied()
    }

    /// Compute the new volume percentage for the focused audio input after
    /// applying `delta` (clamped to 0..=100), returning the input's name and
    /// the new value. Returns `None` when no audio input is focused.
    pub fn adjusted_focused_volume(&self, delta: i16) -> Option<(String, u8)> {
        let a = self.focused_audio()?;
        let current = a.volume_percent.unwrap_or(50) as i16;
        let new_percent = (current + delta).clamp(0, 100) as u8;
        Some((a.name.clone(), new_percent))
    }

    /// Optimistically apply a volume change to the local snapshot so the UI
    /// reflects the new level immediately, before the daemon/OBS round-trip
    /// confirms it. The authoritative value arrives shortly after via an
    /// `InputVolumeChanged` event (which resolves to the same value).
    pub fn set_audio_volume_local(&mut self, name: &str, percent: u8) {
        if let Some(snapshot) = self.snapshot.as_mut()
            && let Some(a) = snapshot.audio_inputs.iter_mut().find(|a| a.name == name)
        {
            let mul = crate::domain::volume::percent_to_mul(percent);
            a.volume_percent = Some(percent);
            a.volume_mul = Some(mul);
            a.volume_db = Some(crate::domain::volume::mul_to_db(mul));
        }
    }

    pub fn focused_profile(&self) -> Option<&str> {
        self.profiles().get(self.profile_cursor).map(String::as_str)
    }

    pub fn focused_scene_collection(&self) -> Option<&str> {
        self.scene_collections()
            .get(self.collection_cursor)
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::state::{ObsSnapshot, SceneState};

    #[test]
    fn focus_panel_grid_navigation_moves_across_rows_and_columns() {
        assert_eq!(FocusPanel::Scenes.right(), FocusPanel::Audio);
        assert_eq!(FocusPanel::Audio.left(), FocusPanel::Scenes);
        assert_eq!(FocusPanel::Profiles.right(), FocusPanel::Collections);
        assert_eq!(FocusPanel::Collections.left(), FocusPanel::Profiles);
        assert_eq!(FocusPanel::Scenes.down(), FocusPanel::Profiles);
        assert_eq!(FocusPanel::Profiles.up(), FocusPanel::Scenes);
        assert_eq!(FocusPanel::Audio.down(), FocusPanel::Collections);
        assert_eq!(FocusPanel::Collections.up(), FocusPanel::Audio);
    }

    #[test]
    fn focus_panel_cycling_wraps_in_both_directions() {
        assert_eq!(FocusPanel::Scenes.next(), FocusPanel::Audio);
        assert_eq!(FocusPanel::Collections.next(), FocusPanel::Scenes);
        assert_eq!(FocusPanel::Scenes.prev(), FocusPanel::Collections);
        assert_eq!(FocusPanel::Audio.prev(), FocusPanel::Scenes);
        // Four Tabs from anywhere return to where they started.
        let mut panel = FocusPanel::Profiles;
        for _ in 0..FocusPanel::CYCLE.len() {
            panel = panel.next();
        }
        assert_eq!(panel, FocusPanel::Profiles);
    }

    #[test]
    fn count_prefix_accumulates_digits_and_saturates() {
        let mut model = TuiModel::default();
        assert_eq!(model.count(), 1, "no typed count means one repetition");

        model.push_count(1);
        model.push_count(2);
        assert_eq!(model.count(), 12);

        for _ in 0..6 {
            model.push_count(9);
        }
        assert_eq!(model.count(), MAX_COUNT as usize);

        model.clear_pending();
        assert_eq!(model.pending_count, None);
        assert_eq!(model.count(), 1);
    }

    #[test]
    fn motions_scale_by_the_count_and_stop_at_the_list_ends() {
        let mut model =
            model_with_scenes((0..10).map(|i| make_scene(&i.to_string(), false)).collect());

        model.move_down_by(4);
        assert_eq!(model.scene_cursor, 4);
        model.move_down_by(100);
        assert_eq!(model.scene_cursor, 9, "clamped to the last row");
        model.move_up_by(3);
        assert_eq!(model.scene_cursor, 6);
        model.move_to_top();
        assert_eq!(model.scene_cursor, 0);
        model.move_to_bottom();
        assert_eq!(model.scene_cursor, 9);
        model.move_up_by(100);
        assert_eq!(model.scene_cursor, 0, "saturates rather than wrapping");
    }

    #[test]
    fn moving_in_an_empty_panel_is_a_no_op() {
        let mut model = TuiModel::default();
        model.move_to_bottom();
        model.move_down_by(5);
        assert_eq!(model.scene_cursor, 0);
    }

    #[test]
    fn log_scrollback_is_bounded_and_returns_to_following() {
        let mut model = TuiModel::default();
        for i in 0..20 {
            model.push_log(TuiLogEntry {
                level: LogLevel::Info,
                message: format!("line {i}"),
                target: None,
                timestamp: OffsetDateTime::UNIX_EPOCH,
            });
        }

        assert_eq!(model.log_view_start(6), 14, "follows the tail by default");

        model.scroll_logs_up(6, 6);
        assert_eq!(model.log_view_start(6), 8);

        // Cannot scroll past the oldest entry we still hold.
        model.scroll_logs_up(1000, 6);
        assert_eq!(model.log_view_start(6), 0);

        // A new entry while scrolled back holds the viewport steady.
        let before = model.logs[model.log_view_start(6)].message.clone();
        model.push_log(TuiLogEntry {
            level: LogLevel::Info,
            message: "line 20".to_string(),
            target: None,
            timestamp: OffsetDateTime::UNIX_EPOCH,
        });
        assert_eq!(model.logs[model.log_view_start(6)].message, before);

        model.scroll_logs_down(1000);
        assert_eq!(model.log_scroll, 0);
        assert_eq!(model.log_view_start(6), 15);
    }

    #[test]
    fn palette_line_editing_never_eats_the_prompt_prefix() {
        let mut palette = CommandPaletteState {
            input: ":scene Main Camera".to_string(),
            ..Default::default()
        };

        palette.delete_word();
        assert_eq!(palette.input, ":scene Main ");
        palette.delete_word();
        assert_eq!(palette.input, ":scene ");
        palette.delete_word();
        assert_eq!(palette.input, ":");
        palette.delete_word();
        assert_eq!(palette.input, ":", "the prefix survives an empty line");

        palette.input = ":vol Mic 70".to_string();
        palette.clear_to_prefix();
        assert_eq!(palette.input, ":");
    }

    #[test]
    fn palette_line_editing_handles_multibyte_input() {
        let mut palette = CommandPaletteState {
            input: ":scene Каме́ра".to_string(),
            ..Default::default()
        };
        palette.delete_word();
        assert_eq!(palette.input, ":scene ");
    }

    #[test]
    fn focus_panel_grid_navigation_is_a_no_op_past_an_edge() {
        assert_eq!(FocusPanel::Scenes.left(), FocusPanel::Scenes);
        assert_eq!(FocusPanel::Scenes.up(), FocusPanel::Scenes);
        assert_eq!(FocusPanel::Audio.right(), FocusPanel::Audio);
        assert_eq!(FocusPanel::Audio.up(), FocusPanel::Audio);
        assert_eq!(FocusPanel::Profiles.left(), FocusPanel::Profiles);
        assert_eq!(FocusPanel::Profiles.down(), FocusPanel::Profiles);
        assert_eq!(FocusPanel::Collections.right(), FocusPanel::Collections);
        assert_eq!(FocusPanel::Collections.down(), FocusPanel::Collections);
    }

    fn make_scene(name: &str, hidden: bool) -> SceneState {
        SceneState {
            name: name.to_string(),
            hidden,
            ..Default::default()
        }
    }

    fn model_with_scenes(scenes: Vec<SceneState>) -> TuiModel {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                scenes,
                ..Default::default()
            }),
            ..Default::default()
        };
        model.clamp_cursors();
        model
    }

    fn model_with_audio(inputs: Vec<AudioState>) -> TuiModel {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                audio_inputs: inputs,
                ..Default::default()
            }),
            focus: FocusPanel::Audio,
            ..Default::default()
        };
        model.clamp_cursors();
        model
    }

    fn audio(name: &str, percent: u8) -> AudioState {
        AudioState {
            name: name.to_string(),
            volume_percent: Some(percent),
            ..Default::default()
        }
    }

    #[test]
    fn adjusted_focused_volume_clamps_to_range() {
        let mut model = model_with_audio(vec![audio("Mic", 98), audio("Desktop", 3)]);

        assert_eq!(
            model.adjusted_focused_volume(5),
            Some(("Mic".to_string(), 100))
        );

        model.audio_cursor = 1;
        assert_eq!(
            model.adjusted_focused_volume(-5),
            Some(("Desktop".to_string(), 0))
        );
    }

    #[test]
    fn adjusted_focused_volume_defaults_when_percent_unknown() {
        let model = model_with_audio(vec![AudioState {
            name: "Mic".to_string(),
            volume_percent: None,
            ..Default::default()
        }]);

        // Falls back to 50 before applying the delta.
        assert_eq!(
            model.adjusted_focused_volume(5),
            Some(("Mic".to_string(), 55))
        );
    }

    #[test]
    fn adjusted_focused_volume_is_none_without_audio() {
        let model = TuiModel::default();
        assert_eq!(model.adjusted_focused_volume(5), None);
    }

    #[test]
    fn set_audio_volume_local_updates_percent_mul_and_db_together() {
        let mut model = model_with_audio(vec![audio("Mic", 50), audio("Desktop", 50)]);

        model.set_audio_volume_local("Desktop", 75);

        let mic = &model.audio_inputs()[0];
        let desktop = &model.audio_inputs()[1];
        // Untargeted input is left untouched.
        assert_eq!(mic.volume_percent, Some(50));
        // Targeted input reflects the new level across all three fields.
        assert_eq!(desktop.volume_percent, Some(75));
        assert_eq!(
            desktop.volume_mul,
            Some(crate::domain::volume::percent_to_mul(75))
        );
        assert!(desktop.volume_db.is_some());
    }

    #[test]
    fn set_audio_volume_local_ignores_unknown_input() {
        let mut model = model_with_audio(vec![audio("Mic", 50)]);
        // Must not panic or mutate anything for a name that isn't present.
        model.set_audio_volume_local("Nonexistent", 10);
        assert_eq!(model.audio_inputs()[0].volume_percent, Some(50));
    }

    #[test]
    fn scenes_excludes_hidden() {
        let model = model_with_scenes(vec![
            make_scene("visible_a", false),
            make_scene("hidden_x", true),
            make_scene("visible_b", false),
            make_scene("hidden_y", true),
        ]);

        let visible: Vec<&str> = model.scenes().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(visible, vec!["visible_a", "visible_b"]);
    }

    #[test]
    fn cursor_stays_zero_when_all_scenes_hidden() {
        let mut model = model_with_scenes(vec![
            make_scene("hidden_a", true),
            make_scene("hidden_b", true),
        ]);
        model.scene_cursor = 5; // set an out-of-bounds cursor before clamping
        model.clamp_cursors();

        assert_eq!(model.scenes().len(), 0);
        assert_eq!(model.scene_cursor, 0);
    }

    #[test]
    fn profile_navigation_clamps_and_focuses() {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                profiles: vec!["Default".to_string(), "Streaming".to_string()],
                ..Default::default()
            }),
            focus: FocusPanel::Profiles,
            ..Default::default()
        };
        model.clamp_cursors();

        assert_eq!(model.focused_profile(), Some("Default"));
        model.move_down();
        assert_eq!(model.focused_profile(), Some("Streaming"));
        model.move_down(); // already at last entry; stays put
        assert_eq!(model.focused_profile(), Some("Streaming"));
        model.move_up();
        assert_eq!(model.focused_profile(), Some("Default"));
    }

    fn stats(fps: f64, render_skipped: u64, render_total: u64) -> ObsStats {
        ObsStats {
            active_fps: fps,
            average_frame_render_time_ms: 4.0,
            render_skipped_frames: render_skipped,
            render_total_frames: render_total,
            output_skipped_frames: render_skipped,
            output_total_frames: render_total,
            ..ObsStats::default()
        }
    }

    fn model_with_stats(streaming: bool, stats: ObsStats) -> TuiModel {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                streaming,
                stats: Some(stats),
                ..Default::default()
            }),
            ..Default::default()
        };
        model.record_metric_sample();
        model
    }

    fn set_stats(model: &mut TuiModel, stats: ObsStats) {
        model.snapshot.as_mut().unwrap().stats = Some(stats);
        model.record_metric_sample();
    }

    #[test]
    fn stream_frame_baseline_latches_on_the_first_sample_of_a_stream() {
        let mut model = model_with_stats(true, stats(60.0, 100, 50_000));

        // The sample that sets the baseline has nothing to measure yet.
        assert_eq!(
            model.stream_frame_baseline,
            Some(FrameCounters {
                render_skipped: 100,
                render_total: 50_000,
                output_skipped: 100,
                output_total: 50_000,
            })
        );
        assert_eq!(model.stream_frame_drops(), None);

        // Later samples report only what this stream lost, not OBS's
        // since-launch totals.
        set_stats(&mut model, stats(60.0, 103, 50_600));
        let drops = model.stream_frame_drops().expect("per-stream drops");
        assert_eq!(drops.render_skipped, 3);
        assert_eq!(drops.render_total, 600);
    }

    #[test]
    fn stream_frame_baseline_clears_when_the_stream_stops() {
        let mut model = model_with_stats(true, stats(60.0, 0, 1_000));
        assert!(model.stream_frame_baseline.is_some());

        model.snapshot.as_mut().unwrap().streaming = false;
        model.record_metric_sample();
        assert_eq!(model.stream_frame_baseline, None);
        assert_eq!(model.stream_frame_drops(), None);

        // Starting a second stream re-baselines against the counters as of
        // that moment rather than reusing the first stream's.
        model.snapshot.as_mut().unwrap().streaming = true;
        set_stats(&mut model, stats(60.0, 7, 9_000));
        assert_eq!(
            model.stream_frame_baseline.map(|b| b.render_total),
            Some(9_000)
        );
    }

    #[test]
    fn stream_frame_drops_survive_counters_resetting_mid_stream() {
        let mut model = model_with_stats(true, stats(60.0, 40, 90_000));
        // OBS restarted: counters go backwards. Saturating subtraction must
        // not wrap into a nonsense drop count.
        set_stats(&mut model, stats(60.0, 1, 500));
        let drops = model.stream_frame_drops();
        assert!(
            drops.is_none_or(|d| d.render_skipped == 0 && d.render_total == 0),
            "counter reset should read as no measurable drops, got {drops:?}"
        );
    }

    #[test]
    fn not_streaming_never_reports_per_stream_drops() {
        let model = model_with_stats(false, stats(60.0, 5, 1_000));
        assert_eq!(model.stream_frame_baseline, None);
        assert_eq!(model.stream_frame_drops(), None);
        assert!(!model.streaming());
    }

    #[test]
    fn frame_and_fps_history_track_each_stats_sample() {
        let mut model = model_with_stats(true, stats(60.0, 0, 100));
        set_stats(&mut model, stats(48.5, 0, 200));

        assert_eq!(model.fps_history.samples(), [60.0, 48.5]);
        assert_eq!(model.frame_time_history.samples(), [4.0, 4.0]);
    }

    /// `RollingSeries` owns the trimming, but the model still has to keep
    /// feeding it — this guards the wiring, not the window arithmetic.
    #[test]
    fn metric_history_is_bounded() {
        let mut model = TuiModel {
            cpu_history: (0..40).map(f64::from).collect(),
            bitrate_history: (0..40).map(f64::from).collect(),
            ..Default::default()
        };
        model.record_metric_sample();
        assert_eq!(model.cpu_history.samples().len(), RollingSeries::CAPACITY);
        assert_eq!(
            model.bitrate_history.samples().len(),
            RollingSeries::CAPACITY
        );
    }

    #[test]
    fn scene_collection_navigation_clamps_and_focuses() {
        let mut model = TuiModel {
            snapshot: Some(ObsSnapshot {
                scene_collections: vec!["Podcast".to_string(), "Gaming".to_string()],
                ..Default::default()
            }),
            focus: FocusPanel::Collections,
            ..Default::default()
        };
        model.clamp_cursors();

        assert_eq!(model.focused_scene_collection(), Some("Podcast"));
        model.move_down();
        assert_eq!(model.focused_scene_collection(), Some("Gaming"));
        model.move_down(); // already at last entry; stays put
        assert_eq!(model.focused_scene_collection(), Some("Gaming"));
        model.move_up();
        assert_eq!(model.focused_scene_collection(), Some("Podcast"));
    }

    #[test]
    fn revealed_last_result_is_none_without_a_result() {
        let model = TuiModel::default();
        assert_eq!(model.revealed_last_result(3), None);
    }

    #[test]
    fn revealed_last_result_grows_with_ticks_then_settles() {
        let mut model = TuiModel::default();
        model.set_last_result("scene set: Main");

        assert_eq!(model.revealed_last_result(3), Some(""));
        model.anim.tick();
        assert_eq!(model.revealed_last_result(3), Some("sce"));
        model.anim.tick();
        assert_eq!(model.revealed_last_result(3), Some("scene "));
        for _ in 0..10 {
            model.anim.tick();
        }
        assert_eq!(model.revealed_last_result(3), Some("scene set: Main"));
    }
}
