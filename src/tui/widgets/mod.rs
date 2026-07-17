pub mod audio;
pub mod chrome;
pub mod collections;
pub mod command_palette;
pub mod connection;
pub mod header;
pub mod live_bar;
pub mod logs;
pub mod profiles;
pub mod scene_map;
pub mod scenes;
pub mod settings;
pub mod splash;

use ratatui::{Frame, style::Style, widgets::Block};

use crate::tui::theme::Theme;

/// Paint the whole frame with the theme's background color. Ratatui only
/// ever touches the cells a widget explicitly styles, so without this fill
/// unstyled space (gaps between panels, empty list rows, ...) would keep
/// showing the terminal emulator's own background instead of the theme's.
/// Must be called first in each `terminal.draw` closure, before any other
/// widget renders on top.
pub fn fill_background(f: &mut Frame, theme: Theme) {
    f.render_widget(
        Block::default().style(Style::default().bg(theme.bg)),
        f.area(),
    );
}
