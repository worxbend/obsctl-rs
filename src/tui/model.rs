use std::collections::{BTreeSet, HashMap};

use time::OffsetDateTime;

use crate::{
    domain::names::{checked_name, normalized_name},
    ipc::protocol::{LogEvent, LogLevel},
    obs::state::{AudioState, ObsSnapshot, ObsStats, SceneProfileState, SceneState, ServerStatus},
    tui::{
        anim::AnimClock,
        input::MAX_COUNT,
        keymap::Pending,
        series::RollingSeries,
        theme::{self, Theme},
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
    pub const ALL: [Self; 4] = [Self::Scenes, Self::Audio, Self::Profiles, Self::Collections];

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
    pub const CYCLE: [Self; 4] = Self::ALL;

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

/// A one-line text field.
///
/// Deliberately not shared with [`CommandPaletteState`]: that one is a command
/// *line*, and every one of its editing rules is written around the prompt
/// character it always keeps at the front — `clear_to_prefix` keeps the first
/// character, which here would leave the first letter of the name behind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextField {
    pub value: String,
}

impl TextField {
    pub fn push(&mut self, c: char) {
        self.value.push(c);
    }

    pub fn backspace(&mut self) {
        self.value.pop();
    }

    pub fn clear(&mut self) {
        self.value.clear();
    }

    /// Ctrl-W — drop the word before the cursor along with the spaces that
    /// trail it, so a second press does not have to eat the gap first.
    pub fn delete_word(&mut self) {
        let end = match self.value.trim_end().rfind(' ') {
            Some(space) => space + 1,
            None => 0,
        };
        self.value.truncate(end);
    }
}

/// Which of the editor's three questions the user is answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneProfileStage {
    /// Which scene profile — an existing one, or a new one.
    Picker,
    /// Which scenes it hides.
    Scenes,
    /// What it is called.
    Naming,
}

/// Why a name typed into the editor was not accepted.
///
/// Checked here rather than left to the daemon because both answers are ones
/// the client already has: the naming rules are shared code, and the list of
/// existing profiles is in the snapshot. Finding out while still typing beats
/// finding out from a status line after the config file has been written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneProfileNameError {
    /// Blank, too long, or otherwise not a usable resource name — the one
    /// thing the daemon would refuse outright.
    Unusable,
    /// Another scene profile already answers to it. Saving would replace that
    /// profile with this one's hidden list, and there is no backup to undo it
    /// with, so the name is refused before anything is sent.
    Taken,
}

impl SceneProfileNameError {
    /// The `locales/en.yml` key naming this to the user.
    pub fn message_key(self) -> &'static str {
        match self {
            Self::Unusable => "tui.panels.scene_profiles.name_blank",
            Self::Taken => "tui.panels.scene_profiles.name_taken",
        }
    }
}

/// The scene-profile editor. `None` on [`TuiModel::scene_profile`] means the
/// modal is closed.
///
/// It carries its own two cursors rather than borrowing one of the dashboard's
/// [`PanelCursors`], for the same reason `settings_cursor` does: the modal is
/// not a panel, and a cursor into the all-scenes list has no business sharing
/// bounds with a cursor into the visible-scenes list underneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneProfileEditor {
    pub stage: SceneProfileStage,
    /// Row in the picker. Row 0 is "new scene profile"; row n+1 is
    /// `snapshot.scene_profiles[n]`.
    pub picker_cursor: usize,
    /// Row in the all-scenes list on the [`SceneProfileStage::Scenes`] stage.
    pub scene_cursor: usize,
    pub name: TextField,
    /// Name to put back when `Esc` abandons a rename mid-edit.
    pub name_before_edit: String,
    /// Scene names this profile hides, as OBS spells them.
    pub hidden: BTreeSet<String>,
    /// The profile being edited, or `None` for one that does not exist yet.
    /// A save whose `name` differs from this is a rename, which the dispatch
    /// carries out as a save followed by a delete of the old name.
    pub editing: Option<String>,
    /// The profile `d` has asked to delete and is waiting for a `y` on, or
    /// `None` when nothing is being confirmed.
    ///
    /// Deleting rewrites the config file and takes no backup, so there is
    /// nothing to undo it with — and `d` sits next to `a`, which is the key a
    /// user reaching for this modal presses most. One keystroke must not be
    /// able to destroy a profile someone hand-built, so the keypress arms this
    /// instead of sending anything, and the footer says whose life is on the
    /// line until it is confirmed or cancelled.
    pub pending_delete: Option<String>,
}

impl Default for SceneProfileEditor {
    fn default() -> Self {
        Self {
            stage: SceneProfileStage::Picker,
            picker_cursor: 0,
            scene_cursor: 0,
            name: TextField::default(),
            name_before_edit: String::new(),
            hidden: BTreeSet::new(),
            editing: None,
            pending_delete: None,
        }
    }
}

impl SceneProfileEditor {
    /// Whether the profile being edited hides `scene`.
    ///
    /// Case-insensitive, because a `hidden:` entry the user typed into the
    /// config file need not match OBS's own spelling of the scene, and the
    /// daemon matches them that way too.
    pub fn hides(&self, scene: &str) -> bool {
        self.hidden
            .iter()
            .any(|hidden| same_scene_profile_name(hidden, scene))
    }
}

/// One row of the scene-profile modal, as the widget draws it and the tests
/// read it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneProfileRow {
    /// Text of the row: a profile name on the picker stage, a scene name on
    /// the others. Empty for [`SceneProfileRowKind::NewProfile`], whose label
    /// is a translated string and therefore the widget's to supply.
    pub label: String,
    /// Whether the editor's cursor is on this row.
    pub selected: bool,
    pub kind: SceneProfileRowKind,
}

/// What a [`SceneProfileRow`] stands for, and everything the widget needs to
/// decide how to draw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneProfileRowKind {
    /// Row 0 of the picker: make a scene profile that does not exist yet.
    NewProfile,
    /// An existing scene profile.
    Profile {
        /// Whether this is the profile currently switched on.
        active: bool,
        /// How many of the names this profile lists are scenes OBS actually
        /// has — that is, how many rows the user will see disappear from the
        /// dashboard when it is switched on.
        hidden_count: usize,
        /// How many names the profile lists in total. Larger than
        /// `hidden_count` when the config still names a scene OBS has since
        /// renamed or deleted, so the row cannot promise to hide more than it
        /// can.
        listed_count: usize,
    },
    /// A scene on the toggle stage.
    Scene {
        /// Whether the profile being edited hides it — what `t` flips.
        hidden: bool,
        /// Whether it is the scene OBS is showing right now.
        current: bool,
    },
    /// A name the profile hides that no scene in the snapshot answers to,
    /// listed after the real scenes on the toggle stage.
    ///
    /// Renaming a scene in OBS leaves the old spelling behind in the config
    /// file, where it hides nothing. Such an entry has no scene to hang a
    /// [`SceneProfileRowKind::Scene`] row off, so without a row of its own it
    /// would be invisible in the editor and written straight back out by the
    /// next save — permanent dead config only a hand edit could clear. `t` on
    /// this row drops the entry.
    MissingScene,
}

/// What the scene-profile cycle key does next, decided by
/// [`TuiModel::next_scene_profile`] and carried out by the action layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneProfileCycle {
    /// The config defines no scene profiles, so there is nothing to cycle
    /// through and the keypress has only an explanation to offer.
    Undefined,
    /// Switch this profile on.
    Activate(String),
    /// Switch scene-profile filtering off, leaving each scene's own `hidden:`
    /// setting to decide again.
    Baseline,
}

/// Whether two names mean the same scene profile. Differences of case or of
/// surrounding whitespace do not, matching how the daemon looks a profile up.
fn same_scene_profile_name(a: &str, b: &str) -> bool {
    match (normalized_name(a), normalized_name(b)) {
        (Ok(a), Ok(b)) => a == b,
        // A name that is not usable at all matches nothing, not even another
        // unusable one: there is no profile it could be naming.
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct TuiModel {
    /// The daemon's view of OBS, or `None` before the first snapshot arrives.
    ///
    /// Private because `cached_visible_scenes` is derived from it and is what
    /// `scenes()` and `focused_scene()` actually read. Assigning this directly
    /// left that cache describing the previous snapshot — so `focused_scene()`
    /// could hand back a scene OBS no longer has, and acting on it would fire
    /// `set_scene` for a name that is gone. Go through [`TuiModel::set_snapshot`],
    /// [`TuiModel::update_snapshot`], or [`TuiModel::clear_snapshot`], which
    /// re-derive it.
    snapshot: Option<ObsSnapshot>,
    pub server_status: Option<ServerStatus>,
    pub logs: Vec<TuiLogEntry>,
    pub command_palette: CommandPaletteState,
    pub last_result: Option<String>,
    /// Tick at which `last_result` was last set; drives the typewriter
    /// reveal animation in the command palette.
    pub last_result_tick: u64,
    pub connected_to_daemon: bool,
    pub focus: FocusPanel,
    /// Where the cursor sits in each panel's list. Read and written through
    /// [`panel_cursor`](TuiModel::panel_cursor) and
    /// [`set_panel_cursor`](TuiModel::set_panel_cursor), which clamp to the
    /// list length — the raw indices are private so no caller can park a
    /// cursor past the end of a list.
    cursors: PanelCursors,
    /// Meter state per input name, folded from `InputVolumeMeters` events by
    /// [`record_meter_level`](TuiModel::record_meter_level) and read back
    /// through [`meter`](TuiModel::meter). Private because the decay and peak
    /// rules live on that write path — a caller inserting a reading itself
    /// would sidestep them.
    meters: HashMap<String, MeterReading>,
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
    /// The scene-profile editor while it is open, `None` while it is not.
    ///
    /// A modal overlay rather than a [`View`]: the dashboard stays on screen
    /// underneath it, so nothing about panel focus, the panel cursors, or the
    /// motions that move them changes while it is up. Every field of it is
    /// written through the `scene_profile_*` methods below, each of which
    /// clamps the cursor of the stage it leaves the editor in.
    pub scene_profile: Option<SceneProfileEditor>,
    /// Cached visible (non-hidden) scenes; rebuilt in `clamp_cursors` after each snapshot update.
    cached_visible_scenes: Vec<SceneState>,
}

/// One cursor per panel, addressed by [`FocusPanel`] so the four indices
/// cannot drift apart or be matched against the wrong panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelCursors([usize; 4]);

impl std::ops::Index<FocusPanel> for PanelCursors {
    type Output = usize;

    fn index(&self, panel: FocusPanel) -> &usize {
        &self.0[panel as usize]
    }
}

impl std::ops::IndexMut<FocusPanel> for PanelCursors {
    fn index_mut(&mut self, panel: FocusPanel) -> &mut usize {
        &mut self.0[panel as usize]
    }
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
            cursors: PanelCursors::default(),
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
            scene_profile: None,
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
    /// Open the palette on a fresh line: the prompt `prefix` followed by
    /// `seed`, which the `<leader>f…` mappings use to jump straight into a
    /// half-typed command (`":scene "`).
    ///
    /// The counterpart to [`close`](CommandPaletteState::close), and here for
    /// the same reason: opening was four field assignments at the call site,
    /// so a field added later could be reset on the way out and forgotten on
    /// the way in.
    pub fn open(&mut self, prefix: char, seed: &str) {
        self.active = true;
        self.input.clear();
        self.input.push(prefix);
        self.input.push_str(seed);
        self.completion_idx = None;
    }

    /// Dismiss the palette, discarding whatever was typed and any completions
    /// offered for it.
    ///
    /// One method rather than four field assignments at each of the two call
    /// sites, so a field added later cannot be reset in one place and not the
    /// other — which would leave a stale completion list to reappear the next
    /// time the palette opens.
    pub fn close(&mut self) {
        self.active = false;
        self.input.clear();
        self.completions.clear();
        self.completion_idx = None;
    }

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

impl TuiLogEntry {
    /// A warning raised by the TUI itself rather than forwarded from the
    /// daemon, so the log pane can show a local problem in the same place the
    /// user already looks for remote ones.
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            level: LogLevel::Warn,
            message: message.into(),
            target: Some("obsctl_rs::tui".to_string()),
            timestamp: OffsetDateTime::now_utc(),
        }
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
    ///
    /// This is the bookkeeping the event loop does after *every* completed
    /// action. What the user's own cancel key does is
    /// [`cancel`](TuiModel::cancel), which is deliberately more than this.
    pub fn clear_pending(&mut self) {
        self.pending = Pending::None;
        self.pending_count = None;
    }

    /// What Esc (and a right-click) does: abandon a half-typed sequence *and*
    /// snap the log pane back to the live tail.
    ///
    /// The tail snap used to sit inline in the action handler, which made
    /// "clear pending" mean two different things depending on whether the user
    /// asked for it or the event loop did it automatically after some other
    /// key. Naming the user-facing one separately keeps the automatic clear
    /// from dragging the log pane back down every time anything happens —
    /// which would make scrolling the log impossible.
    pub fn cancel(&mut self) {
        self.clear_pending();
        self.log_scroll = 0;
    }

    /// Edit the command line and bring its completion list back into step.
    ///
    /// Every edit changes what could be completed, and the recompute used to
    /// be a separate call the five editing actions each had to remember; one
    /// forgotten call left a stale completion menu on screen offering
    /// candidates for a line that is no longer typed. Same mutate-then-
    /// re-derive shape as [`update_snapshot`](TuiModel::update_snapshot).
    pub fn edit_palette(&mut self, edit: impl FnOnce(&mut CommandPaletteState)) {
        edit(&mut self.command_palette);
        let input = self.command_palette.input.clone();
        let completions = crate::tui::completion::compute(&input, self);
        self.command_palette.completions = completions;
        self.command_palette.completion_idx = None;
    }

    /// Open the command line, resolving `prefix` against the configured
    /// `ui.command_palette_prefix` when the binding did not name one.
    pub fn open_palette(&mut self, prefix: Option<char>, seed: &str) {
        let prefix = prefix.unwrap_or(self.palette_prefix);
        self.edit_palette(|palette| palette.open(prefix, seed));
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

    /// The daemon's latest view of OBS, if one has arrived.
    pub fn snapshot(&self) -> Option<&ObsSnapshot> {
        self.snapshot.as_ref()
    }

    /// Read one field off the snapshot, or `default` while none has arrived —
    /// the shape of every trivial accessor below.
    fn snap<'a, T>(&'a self, f: impl FnOnce(&'a ObsSnapshot) -> T, default: T) -> T {
        self.snapshot.as_ref().map(f).unwrap_or(default)
    }

    /// Replace the snapshot and bring everything derived from it up to date.
    pub fn set_snapshot(&mut self, snapshot: ObsSnapshot) {
        self.snapshot = Some(snapshot);
        self.clamp_cursors();
    }

    /// Forget the snapshot — the daemon has gone away, or has no OBS to
    /// describe — and clear what was derived from it.
    pub fn clear_snapshot(&mut self) {
        self.snapshot = None;
        self.clamp_cursors();
    }

    /// Change the snapshot in place, re-deriving afterwards. Does nothing if
    /// no snapshot has arrived yet.
    pub fn update_snapshot(&mut self, edit: impl FnOnce(&mut ObsSnapshot)) {
        if let Some(snapshot) = self.snapshot.as_mut() {
            edit(snapshot);
            self.clamp_cursors();
        }
    }

    /// Visible (non-hidden) scenes, in snapshot order. Returns the cached slice; no allocation per call.
    pub fn scenes(&self) -> &[SceneState] {
        &self.cached_visible_scenes
    }

    pub fn audio_inputs(&self) -> &[AudioState] {
        self.snap(|s| s.audio_inputs.as_slice(), &[])
    }

    pub fn profiles(&self) -> &[String] {
        self.snap(|s| s.profiles.as_slice(), &[])
    }

    pub fn current_profile(&self) -> Option<&str> {
        self.snap(|s| s.current_profile.as_deref(), None)
    }

    pub fn scene_collections(&self) -> &[String] {
        self.snap(|s| s.scene_collections.as_slice(), &[])
    }

    pub fn current_scene_collection(&self) -> Option<&str> {
        self.snap(|s| s.current_scene_collection.as_deref(), None)
    }

    pub fn stats(&self) -> Option<&ObsStats> {
        self.snap(|s| s.stats.as_ref(), None)
    }

    pub fn streaming(&self) -> bool {
        self.snap(|s| s.streaming, false)
    }

    pub fn recording(&self) -> bool {
        self.snap(|s| s.recording, false)
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
        self.snap(|s| s.stream_bitrate_kbps, None)
    }

    pub fn stream_duration_ms(&self) -> Option<u64> {
        self.snap(|s| s.stream_duration_ms, None)
    }

    pub fn record_duration_ms(&self) -> Option<u64> {
        self.snap(|s| s.record_duration_ms, None)
    }

    pub fn current_scene(&self) -> Option<&str> {
        self.snap(|s| s.current_scene.as_deref(), None)
    }

    pub fn obs_connected(&self) -> bool {
        self.snap(|s| s.connected, false)
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
        self.cursors[panel]
    }

    /// Move `panel`'s cursor to `index`, clamped to the list.
    pub fn set_panel_cursor(&mut self, panel: FocusPanel, index: usize) {
        let max = self.panel_len(panel).saturating_sub(1);
        self.cursors[panel] = index.min(max);
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

    /// The four vertical motions (`k`/`j`, `gg`/`G`, `Home`/`End`, the
    /// half-page keys) mean two different things depending on which screen is
    /// up: on the dashboard they move the focused panel's list cursor, and in
    /// the settings view they move the theme cursor and live-preview whatever
    /// it lands on.
    ///
    /// Both meanings are decided here rather than at each of the six actions
    /// that produce a motion, and rather than in a second set of
    /// `SettingsNav*` actions that used to shadow these. One place decides, so
    /// a motion added to the keyboard or the mouse cannot work on one screen
    /// and silently do nothing on the other.
    pub fn nav_up(&mut self, rows: usize) {
        match self.view {
            View::Settings => self.preview_theme(self.settings_cursor.saturating_sub(rows)),
            View::Main => self.move_up_by(rows),
        }
    }

    pub fn nav_down(&mut self, rows: usize) {
        match self.view {
            View::Settings => self.preview_theme(self.settings_cursor.saturating_add(rows)),
            View::Main => self.move_down_by(rows),
        }
    }

    pub fn nav_top(&mut self) {
        match self.view {
            View::Settings => self.preview_theme(0),
            View::Main => self.move_to_top(),
        }
    }

    pub fn nav_bottom(&mut self) {
        match self.view {
            View::Settings => self.preview_theme(usize::MAX),
            View::Main => self.move_to_bottom(),
        }
    }

    /// Enter the settings view, remembering the theme that was active so a
    /// close without confirming can put it back.
    ///
    /// The theme picker is four fields moving together (`view`,
    /// `settings_cursor`, `theme`, `theme_preview_origin`) and its invariant —
    /// `theme_preview_origin` is `Some` exactly while the picker is open — was
    /// spread over four blocks in the event loop. Each transition is one named
    /// method here instead, beside the fields it constrains.
    pub fn open_theme_picker(&mut self) {
        self.theme_preview_origin = Some(self.theme);
        self.settings_cursor = self.theme.index();
        self.view = View::Settings;
    }

    /// Leave the picker without choosing: restore the theme that was active
    /// when it opened, discarding whatever was being previewed.
    pub fn cancel_theme_picker(&mut self) {
        if let Some(original) = self.theme_preview_origin.take() {
            self.theme = original;
        }
        self.view = View::Main;
    }

    /// Confirm the previewed theme and leave the picker. Returns the chosen
    /// theme so the caller can persist it to the config file — that write is
    /// the one part of this transition that is not model state.
    pub fn apply_theme_picker(&mut self) -> Theme {
        let chosen = theme::at(self.settings_cursor);
        self.theme = chosen;
        self.theme_preview_origin = None;
        self.view = View::Main;
        chosen
    }

    /// Move the settings cursor to `index` (clamped to the list of themes)
    /// and make that theme active straight away, so the user judges it on the
    /// whole UI rather than on a swatch. Leaving the picker without
    /// confirming puts the previous theme back — see
    /// [`cancel_theme_picker`](TuiModel::cancel_theme_picker).
    pub fn preview_theme(&mut self, index: usize) {
        self.settings_cursor = index.min(theme::ALL.len().saturating_sub(1));
        self.theme = theme::at(self.settings_cursor);
    }

    /// Keep cursors within valid list bounds; call after snapshot updates.
    pub fn clamp_cursors(&mut self) {
        self.cached_visible_scenes = self
            .snapshot
            .as_ref()
            .map(|s| s.scenes.iter().filter(|sc| !sc.hidden).cloned().collect())
            .unwrap_or_default();
        for panel in FocusPanel::ALL {
            let max = self.panel_len(panel).saturating_sub(1);
            self.cursors[panel] = self.cursors[panel].min(max);
        }
        // The editor's own cursors are otherwise only moved by its
        // transitions, which is fine while it is the only thing changing —
        // but the daemon keeps pushing snapshots at the model while the modal
        // is open, and one that drops a scene (or a profile edited from the
        // CLI) leaves the cursor past the end of the shortened list. No row
        // would be highlighted and `t` would do nothing until the user
        // pressed `j`, so the same clamp the transitions do is re-run here.
        if self.scene_profile.is_some() {
            self.scene_profile_cursor_to(self.scene_profile_cursor());
        }
    }

    pub fn focused_scene(&self) -> Option<&SceneState> {
        self.cached_visible_scenes
            .get(self.cursors[FocusPanel::Scenes])
    }

    pub fn focused_audio(&self) -> Option<&AudioState> {
        self.audio_inputs().get(self.cursors[FocusPanel::Audio])
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
            a.set_level(mul);
        }
    }

    pub fn focused_profile(&self) -> Option<&str> {
        self.profiles()
            .get(self.cursors[FocusPanel::Profiles])
            .map(String::as_str)
    }

    pub fn focused_scene_collection(&self) -> Option<&str> {
        self.scene_collections()
            .get(self.cursors[FocusPanel::Collections])
            .map(String::as_str)
    }

    // --- the scene-profile editor ---
    //
    // Everything below is the only way the editor's fields are written. Each
    // transition leaves the editor on one stage and clamps that stage's
    // cursor, so no caller can park a cursor past the end of a list it did not
    // know had changed underneath it. The lists themselves come from the
    // snapshot, which changes without the editor being touched at all, so
    // `clamp_cursors` re-runs that clamp on every snapshot too.

    /// Every scene the daemon knows about, hidden ones included.
    ///
    /// [`scenes`](TuiModel::scenes) is the visible subset the dashboard draws;
    /// the editor has to show what it is choosing between, which is all of
    /// them.
    pub fn all_scenes(&self) -> &[SceneState] {
        self.snap(|s| s.scenes.as_slice(), &[])
    }

    /// Every scene profile the daemon's config defines.
    pub fn scene_profiles(&self) -> &[SceneProfileState] {
        self.snap(|s| s.scene_profiles.as_slice(), &[])
    }

    /// The scene profile currently switched on, if any.
    pub fn active_scene_profile(&self) -> Option<&str> {
        self.snap(|s| s.active_scene_profile.as_deref(), None)
    }

    /// Where the next press of the cycle key goes, given what is switched on
    /// now. See [`TuiModel::next_scene_profile`].
    ///
    /// A three-state answer rather than an `Option<String>`, because "switch
    /// scene-profile filtering off" and "there is nothing to switch to" are
    /// different keypresses with different outcomes, and an empty name would
    /// have had to stand in for one of them.
    ///
    /// The cycle runs through the profiles in the order the config file lists
    /// them and then through the baseline — no profile at all — before coming
    /// back to the first. The baseline is a stop rather than something to skip
    /// past, since seeing the unfiltered list again is half of what a user
    /// cycling profiles is checking.
    pub fn next_scene_profile(&self) -> SceneProfileCycle {
        let profiles = self.scene_profiles();
        if profiles.is_empty() {
            return SceneProfileCycle::Undefined;
        }
        let position = self.active_scene_profile().and_then(|active| {
            profiles
                .iter()
                .position(|profile| same_scene_profile_name(&profile.name, active))
        });
        match position {
            // Past the last profile is the baseline, not a wrap straight back
            // to the first.
            Some(index) if index + 1 >= profiles.len() => SceneProfileCycle::Baseline,
            Some(index) => SceneProfileCycle::Activate(profiles[index + 1].name.clone()),
            // Either nothing is switched on, or what is switched on names a
            // profile the config no longer defines — a config edited out from
            // under the daemon. Both start the cycle from the top.
            None => SceneProfileCycle::Activate(profiles[0].name.clone()),
        }
    }

    /// Names of the defined scene profiles, for the palette's completion pool.
    pub fn scene_profile_names(&self) -> Vec<String> {
        self.scene_profiles()
            .iter()
            .map(|profile| profile.name.clone())
            .collect()
    }

    /// Open the editor on its picker, with the cursor on the profile the user
    /// is most likely to be here about.
    ///
    /// Row 0 makes a *new* profile, so opening there left every key the footer
    /// advertises — `a` activate, `d` delete, Enter edit — with no profile to
    /// act on, and each of them did nothing at all. Starting on the switched-on
    /// profile (or, when none is switched on, on the first one defined) means
    /// the advertised keys work on the row the editor opens on; row 0 is one
    /// `k` away.
    pub fn open_scene_profiles(&mut self) {
        let active = self.active_scene_profile().and_then(|active| {
            self.scene_profiles()
                .iter()
                .position(|profile| same_scene_profile_name(&profile.name, active))
        });
        // Row n of the profile list is picker row n + 1; row 0 is the only row
        // left to open on when the config defines no profiles at all.
        let row = match (active, self.scene_profiles().is_empty()) {
            (Some(index), _) => index + 1,
            (None, false) => 1,
            (None, true) => 0,
        };
        self.scene_profile = Some(SceneProfileEditor::default());
        self.scene_profile_cursor_to(row);
    }

    /// Close the editor, discarding whatever it was holding.
    pub fn close_scene_profiles(&mut self) {
        self.scene_profile = None;
    }

    /// Rows of the stage that is up, or an empty list when the editor is
    /// closed. The widget draws these and the tests read them, so what the
    /// user sees and what a test asserts cannot come apart.
    pub fn scene_profile_rows(&self) -> Vec<SceneProfileRow> {
        let Some(editor) = self.scene_profile.as_ref() else {
            return Vec::new();
        };
        match editor.stage {
            SceneProfileStage::Picker => {
                let active = self.active_scene_profile();
                let new_row = SceneProfileRow {
                    label: String::new(),
                    selected: editor.picker_cursor == 0,
                    kind: SceneProfileRowKind::NewProfile,
                };
                std::iter::once(new_row)
                    .chain(
                        self.scene_profiles()
                            .iter()
                            .enumerate()
                            .map(|(i, profile)| SceneProfileRow {
                                label: profile.name.clone(),
                                selected: editor.picker_cursor == i + 1,
                                kind: SceneProfileRowKind::Profile {
                                    active: active
                                        .is_some_and(|a| same_scene_profile_name(a, &profile.name)),
                                    hidden_count: self.scenes_matching(&profile.hidden),
                                    listed_count: profile.hidden.len(),
                                },
                            }),
                    )
                    .collect()
            }
            // The naming stage is drawn over the scene list rather than
            // instead of it, so it shows the same rows.
            SceneProfileStage::Scenes | SceneProfileStage::Naming => {
                let current = self.current_scene();
                let scenes =
                    self.all_scenes()
                        .iter()
                        .enumerate()
                        .map(|(i, scene)| SceneProfileRow {
                            label: scene.name.clone(),
                            selected: editor.scene_cursor == i,
                            kind: SceneProfileRowKind::Scene {
                                hidden: editor.hides(&scene.name),
                                current: current.is_some_and(|c| c == scene.name),
                            },
                        });
                // The entries naming nothing OBS has go last, so the rows a
                // user actually chooses between keep the positions they had.
                let offset = self.all_scenes().len();
                let stale = self
                    .scene_profile_stale_hidden()
                    .into_iter()
                    .enumerate()
                    .map(|(i, name)| SceneProfileRow {
                        label: name.to_string(),
                        selected: editor.scene_cursor == offset + i,
                        kind: SceneProfileRowKind::MissingScene,
                    });
                scenes.chain(stale).collect()
            }
        }
    }

    /// The scene profile under the picker cursor, or `None` on row 0 — which
    /// stands for a profile that does not exist yet.
    pub fn selected_scene_profile(&self) -> Option<&SceneProfileState> {
        let editor = self.scene_profile.as_ref()?;
        let index = editor.picker_cursor.checked_sub(1)?;
        self.scene_profiles().get(index)
    }

    /// The profile a delete is waiting to be confirmed for, or `None` when no
    /// delete has been asked for. The widget reads this to draw the prompt and
    /// the key handler reads it to know that the picker's keys mean something
    /// else for one keystroke.
    pub fn scene_profile_pending_delete(&self) -> Option<&str> {
        self.scene_profile
            .as_ref()
            .and_then(|editor| editor.pending_delete.as_deref())
    }

    /// `d` on the picker: arm the delete for the profile under the cursor and
    /// answer with its name, or `None` when the cursor is on the row that
    /// names no profile.
    ///
    /// Nothing is sent here. The delete leaves the config file with one fewer
    /// profile and no backup to restore it from, so it waits for a second,
    /// deliberate keystroke.
    pub fn scene_profile_request_delete(&mut self) -> Option<String> {
        let name = self.selected_scene_profile().map(|p| p.name.clone())?;
        let editor = self.scene_profile.as_mut()?;
        editor.pending_delete = Some(name.clone());
        Some(name)
    }

    /// `y` on the confirmation: hand the name back and disarm, so the caller
    /// can send the delete.
    ///
    /// The name is the one that was armed rather than whatever the cursor is
    /// on now. A snapshot landing between the `d` and the `y` can move the
    /// rows, and the profile the user read in the prompt is the only one they
    /// agreed to lose.
    pub fn scene_profile_confirm_delete(&mut self) -> Option<String> {
        self.scene_profile
            .as_mut()
            .and_then(|editor| editor.pending_delete.take())
    }

    /// Disarm a delete without sending it: `n`, `Esc`, or anything that moves
    /// the cursor off the row the prompt named.
    pub fn scene_profile_cancel_delete(&mut self) {
        if let Some(editor) = self.scene_profile.as_mut() {
            editor.pending_delete = None;
        }
    }

    /// Whether there is a scene list to check a profile's entries against.
    ///
    /// Empty means the TUI has not been told what scenes exist — no snapshot
    /// yet, or a daemon that has not finished talking to OBS. Nothing about a
    /// profile's entries can be judged in that state: every one of them would
    /// look like a name OBS does not have, which is a very different thing
    /// from a name OBS genuinely lost.
    fn scene_list_known(&self) -> bool {
        !self.all_scenes().is_empty()
    }

    /// How many of `hidden` name a scene the snapshot actually has.
    ///
    /// A profile's `hidden` list is config, and config outlives the scenes it
    /// names: a scene renamed in OBS leaves its old spelling behind, hiding
    /// nothing. This is the count that matches what the user will see
    /// disappear, which is the only number the picker may call "hidden".
    ///
    /// With no scene list to check against there is nothing to subtract, so
    /// the entries are counted as they stand: a profile listing two scenes
    /// reads "2 hidden" rather than "0 of 2 hidden", which would be the picker
    /// telling the user their profile had gone stale because the daemon has
    /// not finished connecting.
    fn scenes_matching(&self, hidden: &[String]) -> usize {
        if !self.scene_list_known() {
            return hidden.len();
        }
        hidden
            .iter()
            .filter(|name| {
                self.all_scenes()
                    .iter()
                    .any(|scene| same_scene_profile_name(&scene.name, name))
            })
            .count()
    }

    /// The names the editor is holding that no scene in the snapshot answers
    /// to, in the order they get rows.
    ///
    /// `hidden` is a `BTreeSet`, so this is sorted and stays sorted from one
    /// snapshot to the next — the cursor keeps pointing at the entry it was
    /// on rather than sliding onto its neighbour.
    ///
    /// Empty while no scene list is known. Absence from a list that does not
    /// exist is not evidence of anything, and these rows are an invitation to
    /// press `t` and drop the entry — an invitation that, offered to a user
    /// whose daemon is still connecting, would talk them into deleting a
    /// perfectly good profile one entry at a time.
    fn scene_profile_stale_hidden(&self) -> Vec<&str> {
        let Some(editor) = self.scene_profile.as_ref() else {
            return Vec::new();
        };
        if !self.scene_list_known() {
            return Vec::new();
        }
        editor
            .hidden
            .iter()
            .filter(|hidden| {
                !self
                    .all_scenes()
                    .iter()
                    .any(|scene| same_scene_profile_name(&scene.name, hidden))
            })
            .map(String::as_str)
            .collect()
    }

    /// How many rows the stage that is up has, which is what its cursor is
    /// clamped against.
    fn scene_profile_row_count(&self) -> usize {
        match self.scene_profile.as_ref().map(|editor| editor.stage) {
            Some(SceneProfileStage::Picker) => 1 + self.scene_profiles().len(),
            Some(SceneProfileStage::Scenes | SceneProfileStage::Naming) => {
                self.all_scenes().len() + self.scene_profile_stale_hidden().len()
            }
            None => 0,
        }
    }

    /// The cursor of whichever stage is up, or 0 when the editor is closed.
    fn scene_profile_cursor(&self) -> usize {
        self.scene_profile
            .as_ref()
            .map(|editor| match editor.stage {
                SceneProfileStage::Picker => editor.picker_cursor,
                SceneProfileStage::Scenes | SceneProfileStage::Naming => editor.scene_cursor,
            })
            .unwrap_or(0)
    }

    /// Put the cursor of whichever stage is up on `index`, clamped to that
    /// stage's list. The one place either cursor is assigned.
    fn scene_profile_cursor_to(&mut self, index: usize) {
        let rows = self.scene_profile_row_count();
        let Some(editor) = self.scene_profile.as_mut() else {
            return;
        };
        let clamped = index.min(rows.saturating_sub(1));
        // Moving the cursor answers a pending delete with "no". The prompt
        // names one profile, and once the cursor has left its row the prompt
        // would be asking about a profile that is no longer the one the user
        // is looking at.
        editor.pending_delete = None;
        match editor.stage {
            SceneProfileStage::Picker => editor.picker_cursor = clamped,
            SceneProfileStage::Scenes | SceneProfileStage::Naming => editor.scene_cursor = clamped,
        }
    }

    /// A click in the editor's list: move the cursor of the current stage to
    /// the row that was clicked.
    pub fn scene_profile_set_cursor(&mut self, index: usize) {
        self.scene_profile_cursor_to(index);
    }

    /// Move the cursor of the current stage by `delta` rows `count` times —
    /// `delta` is -1 for `k` and 1 for `j`, and `count` is the typed count
    /// prefix.
    pub fn scene_profile_nav(&mut self, delta: isize, count: usize) {
        if self.scene_profile.is_none() {
            return;
        }
        let cursor = self.scene_profile_cursor();
        let step = delta.saturating_mul(isize::try_from(count).unwrap_or(isize::MAX));
        let moved = isize::try_from(cursor)
            .unwrap_or(isize::MAX)
            .saturating_add(step)
            .max(0);
        self.scene_profile_cursor_to(usize::try_from(moved).unwrap_or(0));
    }

    /// Enter on the picker: row 0 starts a new scene profile, row n edits the
    /// one it names.
    pub fn scene_profile_confirm_picker(&mut self) {
        let on_picker = self
            .scene_profile
            .as_ref()
            .is_some_and(|editor| editor.stage == SceneProfileStage::Picker);
        if !on_picker {
            return;
        }

        let chosen = match self.scene_profile.as_ref().map(|e| e.picker_cursor) {
            Some(0) => None,
            Some(_) => match self.selected_scene_profile() {
                Some(profile) => Some(profile.clone()),
                // The picker is drawn from the snapshot, so a row that is no
                // longer there means a new snapshot landed between the draw
                // and the keypress. Doing nothing beats editing whichever
                // profile happens to have slid into that row.
                None => return,
            },
            None => return,
        };

        let (hidden, name, editing, stage) = match chosen {
            Some(profile) => (
                profile.hidden.iter().cloned().collect(),
                profile.name.clone(),
                Some(profile.name),
                SceneProfileStage::Scenes,
            ),
            // A new profile starts from what the user is already looking at,
            // so saving one straight away cannot silently reveal every scene
            // the config hides today.
            None => (
                self.all_scenes()
                    .iter()
                    .filter(|scene| scene.hidden)
                    .map(|scene| scene.name.clone())
                    .collect(),
                String::new(),
                None,
                SceneProfileStage::Naming,
            ),
        };

        if let Some(editor) = self.scene_profile.as_mut() {
            editor.hidden = hidden;
            editor.name_before_edit = name.clone();
            editor.name = TextField { value: name };
            editor.editing = editing;
            editor.stage = stage;
        }
        self.scene_profile_cursor_to(0);
    }

    /// `n` on the scene stage: start editing the name, remembering the one to
    /// put back if the edit is abandoned.
    pub fn scene_profile_begin_naming(&mut self) {
        if let Some(editor) = self.scene_profile.as_mut()
            && editor.stage == SceneProfileStage::Scenes
        {
            editor.name_before_edit = editor.name.value.clone();
            editor.stage = SceneProfileStage::Naming;
        }
    }

    /// Accept the typed name and go back to the scene list, or say why it was
    /// not accepted — in which case the editor stays on the naming stage with
    /// the name still there to be corrected.
    pub fn scene_profile_commit_name(&mut self) -> std::result::Result<(), SceneProfileNameError> {
        let Some(editor) = self.scene_profile.as_ref() else {
            return Err(SceneProfileNameError::Unusable);
        };
        if editor.stage != SceneProfileStage::Naming {
            return Err(SceneProfileNameError::Unusable);
        }
        let Ok(name) = checked_name(&editor.name.value) else {
            return Err(SceneProfileNameError::Unusable);
        };

        // Compared the way the daemon compares profile names, and against the
        // profile this editor was opened on rather than against the name in
        // the field, so re-confirming a profile's own name is not mistaken
        // for a collision with itself.
        let editing = editor.editing.clone();
        let taken = self.scene_profiles().iter().any(|profile| {
            same_scene_profile_name(&profile.name, &name)
                && !editing
                    .as_deref()
                    .is_some_and(|opened_on| same_scene_profile_name(opened_on, &profile.name))
        });
        if taken {
            return Err(SceneProfileNameError::Taken);
        }

        if let Some(editor) = self.scene_profile.as_mut() {
            editor.name = TextField { value: name };
            editor.stage = SceneProfileStage::Scenes;
        }
        Ok(())
    }

    /// Esc on the naming stage: put the previous name back and return to the
    /// scene list.
    pub fn scene_profile_cancel_name(&mut self) {
        if let Some(editor) = self.scene_profile.as_mut()
            && editor.stage == SceneProfileStage::Naming
        {
            editor.name = TextField {
                value: editor.name_before_edit.clone(),
            };
            editor.stage = SceneProfileStage::Scenes;
        }
    }

    /// Type into the name field. Inert on any other stage, so a stray key
    /// cannot rename a profile the user is only looking at.
    pub fn scene_profile_edit_name(&mut self, edit: impl FnOnce(&mut TextField)) {
        if let Some(editor) = self.scene_profile.as_mut()
            && editor.stage == SceneProfileStage::Naming
        {
            edit(&mut editor.name);
        }
    }

    /// `t` on the scene stage: hide the scene under the cursor, or reveal it
    /// if this profile already hides it.
    pub fn scene_profile_toggle_hidden(&mut self) {
        let on_scenes = self
            .scene_profile
            .as_ref()
            .is_some_and(|editor| editor.stage == SceneProfileStage::Scenes);
        if !on_scenes {
            return;
        }
        let Some(cursor) = self.scene_profile.as_ref().map(|e| e.scene_cursor) else {
            return;
        };
        // Past the last scene are the rows for names the profile hides that
        // OBS has nothing to match — toggling one of those is the only way to
        // get the entry out of the config file.
        let name = match self.all_scenes().get(cursor) {
            Some(scene) => scene.name.clone(),
            None => match cursor
                .checked_sub(self.all_scenes().len())
                .and_then(|stale| self.scene_profile_stale_hidden().get(stale).copied())
            {
                Some(name) => name.to_string(),
                None => return,
            },
        };

        if let Some(editor) = self.scene_profile.as_mut() {
            // Removal goes by the stored spelling, which came from the config
            // file and need not match OBS's, so a toggle cannot leave the same
            // scene listed twice under two casings.
            match editor
                .hidden
                .iter()
                .find(|hidden| same_scene_profile_name(hidden, &name))
                .cloned()
            {
                Some(stored) => {
                    editor.hidden.remove(&stored);
                }
                None => {
                    editor.hidden.insert(name);
                }
            }
        }
        // Dropping the last entry that named a scene OBS no longer has takes
        // its row with it, and the cursor may have been the row that went.
        self.scene_profile_cursor_to(self.scene_profile_cursor());
    }

    /// Esc on the scene stage: back to the picker, keeping the editor open.
    pub fn scene_profile_back(&mut self) {
        if let Some(editor) = self.scene_profile.as_mut()
            && editor.stage == SceneProfileStage::Scenes
        {
            editor.stage = SceneProfileStage::Picker;
        }
        self.scene_profile_cursor_to(
            self.scene_profile
                .as_ref()
                .map(|editor| editor.picker_cursor)
                .unwrap_or(0),
        );
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
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 4);
        model.move_down_by(100);
        assert_eq!(
            model.panel_cursor(FocusPanel::Scenes),
            9,
            "clamped to the last row"
        );
        model.move_up_by(3);
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 6);
        model.move_to_top();
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 0);
        model.move_to_bottom();
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 9);
        model.move_up_by(100);
        assert_eq!(
            model.panel_cursor(FocusPanel::Scenes),
            0,
            "saturates rather than wrapping"
        );
    }

    #[test]
    fn moving_in_an_empty_panel_is_a_no_op() {
        let mut model = TuiModel::default();
        model.move_to_bottom();
        model.move_down_by(5);
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 0);
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

        model.set_panel_cursor(FocusPanel::Audio, 1);
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
        model.set_panel_cursor(FocusPanel::Scenes, 5); // set an out-of-bounds cursor before clamping
        model.clamp_cursors();

        assert_eq!(model.scenes().len(), 0);
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 0);
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
    fn opening_the_theme_picker_remembers_the_current_theme() {
        let mut model = TuiModel::default();
        let original_theme = model.theme;

        model.open_theme_picker();

        assert_eq!(model.view, View::Settings);
        assert_eq!(model.theme_preview_origin, Some(original_theme));
        assert_eq!(model.settings_cursor, original_theme.index());
    }

    /// Previewing then cancelling must leave no trace: the theme goes back and
    /// the "we are previewing" marker is dropped, or the next cancel would
    /// restore a theme from a picker session that ended long ago.
    #[test]
    fn cancelling_the_theme_picker_restores_the_theme_it_opened_with() {
        let mut model = TuiModel::default();
        let original_theme = model.theme;

        model.open_theme_picker();
        model.nav_down(2);
        assert_ne!(model.theme, original_theme, "previewing changes the theme");

        model.cancel_theme_picker();
        assert_eq!(model.theme, original_theme);
        assert_eq!(model.theme_preview_origin, None);
        assert_eq!(model.view, View::Main);
    }

    #[test]
    fn applying_the_theme_picker_keeps_the_previewed_theme() {
        let mut model = TuiModel::default();
        model.open_theme_picker();
        model.nav_bottom();
        let previewed = model.theme;

        let chosen = model.apply_theme_picker();

        assert_eq!(chosen, previewed);
        assert_eq!(model.theme, previewed);
        assert_eq!(model.theme_preview_origin, None);
        assert_eq!(model.view, View::Main);
    }

    /// The same motion means "move the list cursor" on the dashboard and
    /// "move the theme cursor" in the picker, decided in one place.
    #[test]
    fn vertical_motions_follow_the_screen_that_is_up() {
        let mut model =
            model_with_scenes((0..10).map(|i| make_scene(&i.to_string(), false)).collect());

        model.nav_down(3);
        assert_eq!(model.panel_cursor(FocusPanel::Scenes), 3);
        assert_eq!(model.settings_cursor, 0, "the picker is not open");

        model.view = View::Settings;
        model.nav_down(2);
        assert_eq!(model.settings_cursor, 2);
        assert_eq!(
            model.panel_cursor(FocusPanel::Scenes),
            3,
            "the list cursor stays where it was"
        );

        model.nav_top();
        assert_eq!(model.settings_cursor, 0);
        model.nav_bottom();
        assert_eq!(model.settings_cursor, theme::ALL.len() - 1);
    }

    /// Editing the command line re-derives its completions, so no editing key
    /// can leave a menu describing a line that is no longer typed.
    #[test]
    fn editing_the_palette_refreshes_its_completions() {
        let mut model = TuiModel::default();

        model.open_palette(Some(':'), "sce");
        assert_eq!(model.command_palette.input, ":sce");
        assert!(
            model
                .command_palette
                .completions
                .iter()
                .any(|c| c == ":scene"),
            "completions: {:?}",
            model.command_palette.completions
        );

        model.edit_palette(CommandPaletteState::clear_to_prefix);
        assert_eq!(model.command_palette.input, ":");
        assert!(
            model.command_palette.completions.len() > 1,
            "an empty line offers every command"
        );
        assert_eq!(model.command_palette.completion_idx, None);
    }

    /// The configured prompt character is used when a binding does not name
    /// one of its own.
    #[test]
    fn opening_the_palette_falls_back_to_the_configured_prefix() {
        let mut model = TuiModel {
            palette_prefix: '/',
            ..Default::default()
        };
        model.open_palette(None, "");
        assert_eq!(model.command_palette.input, "/");
        assert!(model.command_palette.active);
    }

    /// Esc snaps the log pane back to the live tail; the automatic clear the
    /// event loop does after every action must not, or the pane would jump
    /// back down on the next keypress and scrollback would be unusable.
    #[test]
    fn cancelling_follows_the_log_tail_again_but_clearing_pending_does_not() {
        let mut model = TuiModel::default();
        for i in 0..20 {
            model.push_log(TuiLogEntry {
                level: LogLevel::Info,
                message: format!("line {i}"),
                target: None,
                timestamp: OffsetDateTime::UNIX_EPOCH,
            });
        }
        model.scroll_logs_up(5, 6);
        model.pending = Pending::G;

        model.clear_pending();
        assert_eq!(model.pending, Pending::None);
        assert_eq!(model.log_scroll, 5, "still reading scrolled-back history");

        model.cancel();
        assert_eq!(model.log_scroll, 0);
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

    // --- the scene-profile editor ---

    /// Two scenes, one of them hidden by the config's per-scene flag, and one
    /// scene profile that hides the other one instead.
    fn scene_profile_model() -> TuiModel {
        let mut model = TuiModel::default();
        model.set_snapshot(ObsSnapshot {
            scenes: vec![
                SceneState {
                    name: "Main".to_string(),
                    ..Default::default()
                },
                SceneState {
                    name: "Utility BG".to_string(),
                    hidden: true,
                    ..Default::default()
                },
            ],
            scene_profiles: vec![SceneProfileState {
                name: "streaming".to_string(),
                hidden: vec!["Main".to_string()],
            }],
            active_scene_profile: Some("streaming".to_string()),
            ..Default::default()
        });
        model
    }

    fn labels(rows: &[SceneProfileRow]) -> Vec<&str> {
        rows.iter().map(|row| row.label.as_str()).collect()
    }

    /// The editor shows every scene, not the visible ones the dashboard draws
    /// — choosing what to hide means seeing what is already hidden.
    #[test]
    fn the_scene_stage_lists_scenes_the_dashboard_does_not() {
        let mut model = scene_profile_model();
        assert_eq!(model.scenes().len(), 1, "the dashboard hides 'Main'");

        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();

        let rows = model.scene_profile_rows();
        assert_eq!(labels(&rows), vec!["Main", "Utility BG"]);
        assert_eq!(
            rows[0].kind,
            SceneProfileRowKind::Scene {
                hidden: true,
                current: false
            },
            "the loaded profile hides Main"
        );
        assert_eq!(
            rows[1].kind,
            SceneProfileRowKind::Scene {
                hidden: false,
                current: false
            },
            "and reveals the scene the per-scene flag hides"
        );
    }

    #[test]
    fn toggling_a_scene_hides_it_and_toggling_again_reveals_it() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();

        // Cursor on "Main", which this profile hides.
        model.scene_profile_toggle_hidden();
        assert_eq!(
            model.scene_profile_rows()[0].kind,
            SceneProfileRowKind::Scene {
                hidden: false,
                current: false
            }
        );
        model.scene_profile_toggle_hidden();
        assert_eq!(
            model.scene_profile_rows()[0].kind,
            SceneProfileRowKind::Scene {
                hidden: true,
                current: false
            }
        );
    }

    /// A new profile starts from what is on screen, so saving it straight away
    /// cannot silently reveal every scene the config hides today.
    #[test]
    fn a_new_scene_profile_starts_from_the_scenes_already_hidden() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        // The picker opens on the active profile, so row 0 is one `k` up.
        model.scene_profile_nav(-1, 1);
        model.scene_profile_confirm_picker();

        let editor = model.scene_profile.as_ref().unwrap();
        assert_eq!(editor.stage, SceneProfileStage::Naming);
        assert_eq!(editor.editing, None);
        assert!(editor.name.value.is_empty());
        assert!(editor.hides("Utility BG"));
        assert!(!editor.hides("Main"));
    }

    #[test]
    fn editing_an_existing_scene_profile_copies_it_whole() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();

        let editor = model.scene_profile.as_ref().unwrap();
        assert_eq!(editor.stage, SceneProfileStage::Scenes);
        assert_eq!(editor.editing.as_deref(), Some("streaming"));
        assert_eq!(editor.name.value, "streaming");
        assert!(editor.hides("Main"));
    }

    #[test]
    fn the_picker_marks_the_active_scene_profile_and_counts_what_it_hides() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();

        let rows = model.scene_profile_rows();
        assert_eq!(rows.len(), 2, "the new-profile row plus the one defined");
        assert_eq!(rows[0].kind, SceneProfileRowKind::NewProfile);
        assert_eq!(rows[1].label, "streaming");
        assert_eq!(
            rows[1].kind,
            SceneProfileRowKind::Profile {
                active: true,
                hidden_count: 1,
                listed_count: 1
            }
        );
    }

    /// The keys the picker's footer advertises — `a`, `d`, Enter — all act on
    /// the profile under the cursor, so opening on the row that names no
    /// profile left every one of them inert. The editor opens on the profile
    /// that is switched on instead.
    #[test]
    fn the_picker_opens_on_the_active_scene_profile() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();

        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 1);
        assert_eq!(
            model.selected_scene_profile().map(|p| p.name.as_str()),
            Some("streaming"),
            "so the very next `a` has a profile to activate"
        );
        let rows = model.scene_profile_rows();
        assert!(rows[1].selected);
        assert!(!rows[0].selected, "and the new-profile row is not the one");

        // Row 0 is still one `k` away.
        model.scene_profile_nav(-1, 1);
        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 0);
        assert_eq!(model.selected_scene_profile(), None);
    }

    /// With nothing switched on there is still a profile the user came here
    /// for; with no profiles at all the only row there is is the one that
    /// makes one.
    #[test]
    fn the_picker_opens_on_the_first_profile_when_none_is_active() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| snapshot.active_scene_profile = None);
        model.open_scene_profiles();
        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 1);

        let mut empty = scene_profile_model();
        empty.update_snapshot(|snapshot| {
            snapshot.scene_profiles.clear();
            snapshot.active_scene_profile = None;
        });
        empty.open_scene_profiles();
        assert_eq!(empty.scene_profile.as_ref().unwrap().picker_cursor, 0);
        assert_eq!(
            empty.scene_profile_rows()[0].kind,
            SceneProfileRowKind::NewProfile
        );
    }

    /// An `active_scene_profile` naming a profile the config no longer defines
    /// is stale state the daemon can hand over; the picker must not open on a
    /// row that does not exist.
    #[test]
    fn an_active_profile_that_is_not_defined_does_not_move_the_cursor_past_the_rows() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.active_scene_profile = Some("gone".to_string());
        });
        model.open_scene_profiles();

        assert_eq!(
            model.scene_profile.as_ref().unwrap().picker_cursor,
            1,
            "the first profile defined, not a row off the end"
        );
    }

    /// A profile that still names a scene OBS has renamed away hides fewer
    /// scenes than it lists, and the picker has to say the number the user can
    /// check against the dashboard.
    #[test]
    fn the_picker_counts_only_the_hidden_names_obs_still_has_a_scene_for() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.scene_profiles[0]
                .hidden
                .push("Renamed Away".to_string());
        });
        model.open_scene_profiles();

        assert_eq!(
            model.scene_profile_rows()[1].kind,
            SceneProfileRowKind::Profile {
                active: true,
                hidden_count: 1,
                listed_count: 2
            }
        );
    }

    /// A `hidden:` entry naming no scene OBS has gets a row of its own. Without
    /// one it could not be seen or toggled, and the next save wrote it straight
    /// back out — config that hides nothing and that only a hand edit cleared.
    #[test]
    fn a_hidden_entry_obs_has_no_scene_for_is_listed_and_can_be_dropped() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.scene_profiles[0]
                .hidden
                .push("Renamed Away".to_string());
        });
        model.open_scene_profiles();
        model.scene_profile_confirm_picker();

        let rows = model.scene_profile_rows();
        assert_eq!(
            labels(&rows),
            vec!["Main", "Utility BG", "Renamed Away"],
            "the stale entry goes after the scenes that do exist"
        );
        assert_eq!(rows[2].kind, SceneProfileRowKind::MissingScene);

        // `t` on that row drops it, which is what takes it out of the payload
        // the next save sends.
        model.scene_profile_set_cursor(2);
        model.scene_profile_toggle_hidden();
        assert!(!model.scene_profile.as_ref().unwrap().hides("Renamed Away"));
        assert_eq!(
            labels(&model.scene_profile_rows()),
            vec!["Main", "Utility BG"],
            "and the row goes with it"
        );
        assert_eq!(
            model.scene_profile.as_ref().unwrap().scene_cursor,
            1,
            "the cursor lands on the last row that is left, not past the end"
        );
    }

    /// With no scene list to compare against, nothing about a profile's
    /// entries is knowable — and the editor must not pretend otherwise.
    ///
    /// A TUI whose daemon has not finished talking to OBS holds a snapshot
    /// with no scenes in it. Judging the entries by absence from that list
    /// marked every one of them "not a scene OBS has — press t to drop it",
    /// which is an invitation to dismantle a perfectly good profile while OBS
    /// is merely disconnected.
    #[test]
    fn nothing_is_stale_while_the_scene_list_is_unknown() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.connected = false;
            snapshot.scenes.clear();
        });
        model.open_scene_profiles();

        // The picker counts the entries as the file lists them rather than
        // reporting "0 of 1 hidden", which would read as a broken profile.
        assert_eq!(
            model.scene_profile_rows()[1].kind,
            SceneProfileRowKind::Profile {
                active: true,
                hidden_count: 1,
                listed_count: 1
            }
        );

        // And the toggle stage offers no rows at all rather than one "drop
        // this entry" row per name the profile holds.
        model.scene_profile_confirm_picker();
        assert!(model.scene_profile.as_ref().unwrap().hides("Main"));
        assert!(
            model.scene_profile_rows().is_empty(),
            "no scenes are known, so there is nothing to list — least of all a
             row inviting the user to delete an entry"
        );

        // `t` with the cursor nowhere in particular cannot drop the entry the
        // profile is holding either.
        model.scene_profile_toggle_hidden();
        assert!(
            model.scene_profile.as_ref().unwrap().hides("Main"),
            "the entry survives a keypress made against an unknown scene list"
        );
    }

    /// The cycle key walks the profiles in the order the config lists them,
    /// stops off at the unfiltered list, and only then comes back round. Two
    /// profiles therefore take three presses to return to where they started.
    #[test]
    fn the_cycle_key_visits_every_profile_and_the_unfiltered_list() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.scene_profiles.push(SceneProfileState {
                name: "recording".to_string(),
                hidden: Vec::new(),
            });
        });

        // "streaming" is switched on, so the next stop is the profile after it.
        assert_eq!(
            model.next_scene_profile(),
            SceneProfileCycle::Activate("recording".to_string())
        );

        model.update_snapshot(|snapshot| {
            snapshot.active_scene_profile = Some("recording".to_string());
        });
        assert_eq!(
            model.next_scene_profile(),
            SceneProfileCycle::Baseline,
            "past the last profile is no profile at all, not a wrap"
        );

        model.update_snapshot(|snapshot| snapshot.active_scene_profile = None);
        assert_eq!(
            model.next_scene_profile(),
            SceneProfileCycle::Activate("streaming".to_string()),
            "and from there back to the first"
        );
    }

    /// With nothing to cycle through, the keypress has to say so — a key that
    /// silently does nothing is the defect this whole feature started with.
    #[test]
    fn the_cycle_key_has_nowhere_to_go_without_profiles() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.scene_profiles.clear();
            snapshot.active_scene_profile = None;
        });
        assert_eq!(model.next_scene_profile(), SceneProfileCycle::Undefined);

        // And with no snapshot at all — the TUI before the daemon answers.
        assert_eq!(
            TuiModel::default().next_scene_profile(),
            SceneProfileCycle::Undefined
        );
    }

    /// An `active_scene_profile` the config no longer defines leaves the cycle
    /// with no position to advance from; starting over at the first profile is
    /// the only step that reaches somewhere real.
    #[test]
    fn the_cycle_starts_over_when_the_active_profile_is_not_defined() {
        let mut model = scene_profile_model();
        model.update_snapshot(|snapshot| {
            snapshot.active_scene_profile = Some("gone".to_string());
        });
        assert_eq!(
            model.next_scene_profile(),
            SceneProfileCycle::Activate("streaming".to_string())
        );
    }

    /// Each transition clamps the cursor of the stage it leaves the editor on,
    /// so neither cursor can point past its own list.
    #[test]
    fn editor_cursors_stay_inside_their_own_lists() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();

        model.scene_profile_nav(1, 99);
        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 1);
        model.scene_profile_nav(-1, 99);
        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 0);

        model.scene_profile_set_cursor(99);
        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 1);

        model.scene_profile_confirm_picker();
        model.scene_profile_set_cursor(99);
        assert_eq!(
            model.scene_profile.as_ref().unwrap().scene_cursor,
            1,
            "the scene list has two rows, not two profiles"
        );
    }

    #[test]
    fn a_blank_name_is_refused_and_keeps_the_user_on_the_naming_stage() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(-1, 1);
        model.scene_profile_confirm_picker();

        model.scene_profile_edit_name(|name| name.push(' '));
        assert_eq!(
            model.scene_profile_commit_name(),
            Err(SceneProfileNameError::Unusable)
        );
        assert_eq!(
            model.scene_profile.as_ref().unwrap().stage,
            SceneProfileStage::Naming
        );

        for c in "night".chars() {
            model.scene_profile_edit_name(|name| name.push(c));
        }
        assert_eq!(model.scene_profile_commit_name(), Ok(()));
        let editor = model.scene_profile.as_ref().unwrap();
        assert_eq!(editor.stage, SceneProfileStage::Scenes);
        assert_eq!(editor.name.value, "night", "committing trims the name");
    }

    /// Naming a new profile after one that already exists would have the
    /// daemon replace that profile — an upsert is what `save_scene_profile`
    /// is — so the name is refused while the user is still typing it.
    #[test]
    fn a_name_another_scene_profile_already_uses_is_refused() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(-1, 1);
        model.scene_profile_confirm_picker();

        for c in "STREAMING".chars() {
            model.scene_profile_edit_name(|name| name.push(c));
        }
        assert_eq!(
            model.scene_profile_commit_name(),
            Err(SceneProfileNameError::Taken),
            "matching is case-insensitive, the way the daemon matches"
        );
        assert_eq!(
            model.scene_profile.as_ref().unwrap().stage,
            SceneProfileStage::Naming,
            "and the user stays where they can fix it"
        );
    }

    /// Re-confirming a profile's own name is not a collision with itself.
    #[test]
    fn a_profile_may_be_saved_under_the_name_it_already_has() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();

        model.scene_profile_begin_naming();
        assert_eq!(model.scene_profile_commit_name(), Ok(()));
        assert_eq!(
            model.scene_profile.as_ref().unwrap().name.value,
            "streaming"
        );
    }

    /// The editor's cursors are moved by its own transitions, but the lists it
    /// shows come from the snapshot — which the daemon keeps pushing while the
    /// modal is open. A snapshot that drops a scene must not leave the cursor
    /// past the end, where no row is highlighted and `t` does nothing.
    #[test]
    fn a_shorter_snapshot_reclamps_the_open_editors_cursor() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();
        model.scene_profile_nav(1, 1);
        assert_eq!(model.scene_profile.as_ref().unwrap().scene_cursor, 1);

        model.update_snapshot(|snapshot| {
            snapshot.scenes.truncate(1);
        });

        assert_eq!(model.scene_profile.as_ref().unwrap().scene_cursor, 0);
        let rows = model.scene_profile_rows();
        assert_eq!(rows.len(), 1);
        assert!(
            rows.iter().any(|row| row.selected),
            "a row is still highlighted"
        );

        // And the toggle acts on that row rather than doing nothing. The
        // editor was opened on "streaming", which hides "Main", so the toggle
        // reveals it.
        assert!(model.scene_profile.as_ref().unwrap().hides("Main"));
        model.scene_profile_toggle_hidden();
        assert!(!model.scene_profile.as_ref().unwrap().hides("Main"));
    }

    /// The same gap on the picker: a profile deleted from elsewhere while the
    /// picker is open must not leave the cursor pointing past the last row,
    /// where activate and delete would both be inert.
    #[test]
    fn a_snapshot_that_drops_a_profile_reclamps_the_picker_cursor() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        assert!(model.selected_scene_profile().is_some());

        model.update_snapshot(|snapshot| {
            snapshot.scene_profiles.clear();
        });

        assert_eq!(model.scene_profile.as_ref().unwrap().picker_cursor, 0);
        assert!(
            model.scene_profile_rows().iter().any(|row| row.selected),
            "the 'new scene profile' row is highlighted instead of nothing"
        );
    }

    #[test]
    fn abandoning_a_rename_puts_the_previous_name_back() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();

        model.scene_profile_begin_naming();
        model.scene_profile_edit_name(TextField::clear);
        model.scene_profile_edit_name(|name| name.push('x'));
        model.scene_profile_cancel_name();

        let editor = model.scene_profile.as_ref().unwrap();
        assert_eq!(editor.stage, SceneProfileStage::Scenes);
        assert_eq!(editor.name.value, "streaming");
    }

    #[test]
    fn esc_on_the_scene_list_returns_to_the_picker_without_closing() {
        let mut model = scene_profile_model();
        model.open_scene_profiles();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();

        model.scene_profile_back();
        assert_eq!(
            model.scene_profile.as_ref().unwrap().stage,
            SceneProfileStage::Picker
        );
        model.close_scene_profiles();
        assert!(model.scene_profile.is_none());
        assert!(model.scene_profile_rows().is_empty());
    }

    /// The editor's transitions are all no-ops while it is closed, so a stray
    /// action cannot bring half of it back.
    #[test]
    fn editor_transitions_do_nothing_while_the_editor_is_closed() {
        let mut model = scene_profile_model();
        model.scene_profile_nav(1, 1);
        model.scene_profile_confirm_picker();
        model.scene_profile_toggle_hidden();
        model.scene_profile_begin_naming();
        assert_eq!(
            model.scene_profile_commit_name(),
            Err(SceneProfileNameError::Unusable)
        );
        assert!(model.scene_profile.is_none());
    }

    #[test]
    fn deleting_a_word_from_a_text_field_takes_the_spaces_with_it() {
        let mut field = TextField {
            value: "late night  ".to_string(),
        };
        field.delete_word();
        assert_eq!(field.value, "late ");
        field.delete_word();
        assert_eq!(field.value, "");
        field.delete_word();
        assert_eq!(field.value, "");
    }
}
