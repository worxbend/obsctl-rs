use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

pub struct LayoutAreas {
    pub header: Rect,
    pub left_top: Rect,
    pub left_bottom: Rect,
    pub right: Rect,
    pub palette: Rect,
}

pub fn compute(frame: &Frame) -> LayoutAreas {
    let area = frame.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(5),
        ])
        .split(area);

    let header = vertical[0];
    let middle = vertical[1];
    let palette = vertical[2];

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(middle);

    let left = horizontal[0];
    let right = horizontal[1];

    let left_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(left);

    LayoutAreas {
        header,
        left_top: left_split[0],
        left_bottom: left_split[1],
        right,
        palette,
    }
}
