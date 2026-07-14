use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

pub struct LayoutAreas {
    pub header: Rect,
    /// Animated LIVE/REC banner + CPU/FPS/bitrate readout.
    pub live_bar: Rect,
    pub scenes: Rect,
    pub audio: Rect,
    pub profiles: Rect,
    pub collections: Rect,
    /// Slim strip under the main panels — deliberately short so logs stay
    /// out of the way of the scenes/audio/profiles/collections dashboard.
    pub logs: Rect,
    pub palette: Rect,
}

/// Minimum height (in rows) reserved for the profiles/collections row,
/// deliberately small since those lists are usually short — the
/// scenes/audio row above it (`Constraint::Min`) absorbs any extra space.
const PROFILES_COLLECTIONS_ROW_HEIGHT: u16 = 7;

pub fn compute(frame: &Frame) -> LayoutAreas {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(8),
            Constraint::Length(5),
        ])
        .split(area);

    let header = vertical[0];
    let live_bar = vertical[1];
    let middle = vertical[2];
    let logs = vertical[3];
    let palette = vertical[4];

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
        scenes: scenes_audio_cols[0],
        audio: scenes_audio_cols[1],
        profiles: profiles_collections_cols[0],
        collections: profiles_collections_cols[1],
        logs,
        palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn compute_for(width: u16, height: u16) -> LayoutAreas {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut areas = None;
        terminal
            .draw(|f| {
                areas = Some(compute(f));
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
    fn header_live_bar_logs_and_palette_are_unchanged() {
        let areas = compute_for(100, 40);
        assert_eq!(areas.header.height, 3);
        assert_eq!(areas.live_bar.height, 3);
        assert_eq!(areas.logs.height, 8);
        assert_eq!(areas.palette.height, 5);
    }
}
