use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

pub struct LayoutAreas {
    pub header: Rect,
    /// Animated LIVE/REC banner + CPU/FPS/bitrate readout.
    pub live_bar: Rect,
    /// Oversized IDLE/LIVE/REC pane in the top-right corner, spanning the
    /// header and live-bar rows. `None` on terminals too narrow to give it
    /// its width without squeezing the header — the live bar then falls back
    /// to its inline badges so the state is never hidden.
    pub status: Option<Rect>,
    pub scenes: Rect,
    pub audio: Rect,
    pub profiles: Rect,
    pub collections: Rect,
    /// Slim strip under the main panels — deliberately short so logs stay
    /// out of the way of the scenes/audio/profiles/collections dashboard.
    pub logs: Rect,
    /// Stream health readout, carved out of the right of the logs strip
    /// while streaming. `None` when idle, or when the terminal is too
    /// narrow to show it without squeezing the logs into uselessness.
    pub stats: Option<Rect>,
    pub palette: Rect,
}

/// Minimum height (in rows) reserved for the profiles/collections row,
/// deliberately small since those lists are usually short — the
/// scenes/audio row above it (`Constraint::Min`) absorbs any extra space.
const PROFILES_COLLECTIONS_ROW_HEIGHT: u16 = 7;

/// Fixed width of the stats pane. Its rows are label + value + graph, so a
/// percentage split would either clip them on small terminals or waste half
/// a wide one; the logs list absorbs all remaining width instead.
const STATS_PANE_WIDTH: u16 = 46;

/// Below this the logs list degrades into unreadable slivers, so the stats
/// pane yields the whole strip back to it.
const LOGS_MIN_WIDTH: u16 = 30;

/// Width of the top-right broadcast-state pane. Sized to the widest thing it
/// draws — `LIVE` and `REC` side by side in the block font, 15 + 2 + 11
/// columns — plus borders and a column of breathing room each side. Every
/// column past that is taken straight off the header, which needs it to spell
/// out the scene and profile names.
pub const STATUS_PANE_WIDTH: u16 = 32;

/// The header still has to spell out daemon/OBS/scene/profile; below this it
/// is better to drop the status pane than to truncate all of that.
const CHROME_MIN_WIDTH: u16 = 44;

/// `show_stats` requests the stream-health pane (the caller passes whether
/// OBS is streaming); the returned `stats` area is still `None` when the
/// terminal cannot spare the width.
pub fn compute(frame: &Frame, show_stats: bool) -> LayoutAreas {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(7),
            Constraint::Length(4),
        ])
        .split(area);

    let middle = vertical[2];
    let logs_strip = vertical[3];
    let palette = vertical[4];

    // The status pane occupies the top-right corner across both chrome rows,
    // so the header and live bar give up the same slice of width.
    let (header, live_bar, status) = if area.width >= STATUS_PANE_WIDTH + CHROME_MIN_WIDTH {
        let split = |row: Rect| {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(CHROME_MIN_WIDTH),
                    Constraint::Length(STATUS_PANE_WIDTH),
                ])
                .split(row)
        };
        let header_cols = split(vertical[0]);
        let live_bar_cols = split(vertical[1]);
        let status = Rect {
            x: header_cols[1].x,
            y: header_cols[1].y,
            width: header_cols[1].width,
            height: vertical[0].height + vertical[1].height,
        };
        (header_cols[0], live_bar_cols[0], Some(status))
    } else {
        (vertical[0], vertical[1], None)
    };

    let (logs, stats) = if show_stats && logs_strip.width >= STATS_PANE_WIDTH + LOGS_MIN_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(LOGS_MIN_WIDTH),
                Constraint::Length(STATS_PANE_WIDTH),
            ])
            .split(logs_strip);
        (cols[0], Some(cols[1]))
    } else {
        (logs_strip, None)
    };

    // Middle dashboard: a bigger scenes/audio row on top, a smaller
    // profiles/collections row underneath.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(PROFILES_COLLECTIONS_ROW_HEIGHT),
        ])
        .split(middle);
    let scenes_audio_row = rows[0];
    let profiles_collections_row = rows[1];

    let scenes_audio_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(scenes_audio_row);

    let profiles_collections_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(profiles_collections_row);

    LayoutAreas {
        header,
        live_bar,
        status,
        scenes: scenes_audio_cols[0],
        audio: scenes_audio_cols[1],
        profiles: profiles_collections_cols[0],
        collections: profiles_collections_cols[1],
        logs,
        stats,
        palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn compute_for(width: u16, height: u16) -> LayoutAreas {
        compute_with_stats(width, height, false)
    }

    fn compute_with_stats(width: u16, height: u16, show_stats: bool) -> LayoutAreas {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut areas = None;
        terminal
            .draw(|f| {
                areas = Some(compute(f, show_stats));
            })
            .unwrap();
        areas.unwrap()
    }

    #[test]
    fn scenes_audio_row_is_bigger_than_profiles_collections_row() {
        let areas = compute_for(100, 40);
        assert!(areas.scenes.height > areas.profiles.height);
        assert_eq!(areas.scenes.height, areas.audio.height);
        assert_eq!(areas.profiles.height, areas.collections.height);
        assert_eq!(areas.profiles.height, PROFILES_COLLECTIONS_ROW_HEIGHT);
    }

    #[test]
    fn scenes_audio_and_profiles_collections_are_side_by_side_columns() {
        let areas = compute_for(100, 40);
        assert_eq!(areas.scenes.y, areas.audio.y);
        assert!(areas.audio.x >= areas.scenes.x + areas.scenes.width);
        assert_eq!(areas.profiles.y, areas.collections.y);
        assert!(areas.collections.x >= areas.profiles.x + areas.profiles.width);
        // Scenes/audio row sits directly above the profiles/collections row.
        assert_eq!(areas.profiles.y, areas.scenes.y + areas.scenes.height);
    }

    #[test]
    fn logs_strip_keeps_full_width_when_not_streaming() {
        let areas = compute_for(120, 40);
        assert!(areas.stats.is_none());
        assert_eq!(areas.logs.width, 120);
    }

    #[test]
    fn streaming_splits_the_logs_strip_with_the_stats_pane() {
        let areas = compute_with_stats(120, 40, true);
        let stats = areas.stats.expect("stats pane while streaming");

        assert_eq!(stats.width, STATS_PANE_WIDTH);
        assert_eq!(stats.x, areas.logs.x + areas.logs.width);
        // Same strip: both sit on the logs row, at the same height.
        assert_eq!(stats.y, areas.logs.y);
        assert_eq!(stats.height, areas.logs.height);
        assert_eq!(areas.logs.width + stats.width, 120);
    }

    #[test]
    fn narrow_terminals_keep_logs_instead_of_the_stats_pane() {
        // One column short of fitting both panes.
        let areas = compute_with_stats(STATS_PANE_WIDTH + LOGS_MIN_WIDTH - 1, 40, true);
        assert!(areas.stats.is_none());
        assert_eq!(areas.logs.width, STATS_PANE_WIDTH + LOGS_MIN_WIDTH - 1);

        // Exactly wide enough: both appear, logs at its minimum.
        let areas = compute_with_stats(STATS_PANE_WIDTH + LOGS_MIN_WIDTH, 40, true);
        assert!(areas.stats.is_some());
        assert_eq!(areas.logs.width, LOGS_MIN_WIDTH);
    }

    #[test]
    fn chrome_rows_have_room_for_rich_status_content() {
        let areas = compute_for(100, 40);
        assert_eq!(areas.header.height, 4);
        assert_eq!(areas.live_bar.height, 4);
        assert_eq!(areas.logs.height, 7);
        assert_eq!(areas.palette.height, 4);
    }

    #[test]
    fn status_pane_sits_in_the_top_right_corner_across_both_chrome_rows() {
        let areas = compute_for(120, 40);
        let status = areas.status.expect("status pane on a wide terminal");

        assert_eq!(status.width, STATUS_PANE_WIDTH);
        // Flush against the top-right corner.
        assert_eq!(status.y, 0);
        assert_eq!(status.x + status.width, 120);
        // Spans the header and live-bar rows as one pane.
        assert_eq!(status.height, areas.header.height + areas.live_bar.height);
        // The chrome rows yield exactly that width, and nothing overlaps.
        assert_eq!(areas.header.x + areas.header.width, status.x);
        assert_eq!(areas.live_bar.x + areas.live_bar.width, status.x);
        assert_eq!(areas.header.width, 120 - STATUS_PANE_WIDTH);
        assert_eq!(areas.live_bar.width, 120 - STATUS_PANE_WIDTH);
    }

    #[test]
    fn narrow_terminals_drop_the_status_pane_and_keep_full_width_chrome() {
        // One column short of fitting both.
        let areas = compute_for(STATUS_PANE_WIDTH + CHROME_MIN_WIDTH - 1, 40);
        assert!(areas.status.is_none());
        assert_eq!(areas.header.width, STATUS_PANE_WIDTH + CHROME_MIN_WIDTH - 1);
        assert_eq!(
            areas.live_bar.width,
            STATUS_PANE_WIDTH + CHROME_MIN_WIDTH - 1
        );

        // Exactly wide enough: the pane appears, the header at its minimum.
        let areas = compute_for(STATUS_PANE_WIDTH + CHROME_MIN_WIDTH, 40);
        assert!(areas.status.is_some());
        assert_eq!(areas.header.width, CHROME_MIN_WIDTH);
    }

    #[test]
    fn status_pane_is_wide_enough_for_live_and_rec_side_by_side() {
        use crate::tui::spinner;

        let inner = STATUS_PANE_WIDTH as usize - 2; // borders
        let both = spinner::block_width("LIVE") + 2 + spinner::block_width("REC");
        assert!(
            both <= inner,
            "LIVE+REC needs {both} columns but the pane only has {inner}"
        );
        assert!(spinner::block_width("IDLE") <= inner);
    }
}
