use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

pub struct LayoutAreas {
    pub header: Rect,
    pub scenes: Rect,
    pub audio: Rect,
    pub profiles: Rect,
    /// Slim strip under the main panels — deliberately short so logs stay
    /// out of the way of the scenes/audio/profiles dashboard.
    pub logs: Rect,
    pub palette: Rect,
}

pub fn compute(frame: &Frame) -> LayoutAreas {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(8),
            Constraint::Length(5),
        ])
        .split(area);

    let header = vertical[0];
    let middle = vertical[1];
    let logs = vertical[2];
    let palette = vertical[3];

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(middle);

    LayoutAreas {
        header,
        scenes: columns[0],
        audio: columns[1],
        profiles: columns[2],
        logs,
        palette,
    }
}
