//! Color themes for the TUI, selectable via `ui.theme` in config or the
//! in-app settings view (styled after btop's theme switcher).

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub label: &'static str,
    /// App name / primary brand accent.
    pub accent: Color,
    /// Secondary accent (links, alt highlights).
    pub accent_alt: Color,
    /// Default body text.
    pub fg: Color,
    /// Secondary / dimmed text.
    pub muted: Color,
    /// Unfocused panel border.
    pub border: Color,
    /// Focused panel border.
    pub border_focus: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    /// Selected-row background/foreground.
    pub highlight_bg: Color,
    pub highlight_fg: Color,
}

macro_rules! rgb {
    ($r:expr, $g:expr, $b:expr) => {
        Color::Rgb($r, $g, $b)
    };
}

const CLAUDE: Theme = Theme {
    id: "claude",
    label: "Claude",
    accent: rgb!(0xD9, 0x77, 0x57),
    accent_alt: rgb!(0xE8, 0xC5, 0x9E),
    fg: rgb!(0xEC, 0xE8, 0xE1),
    muted: rgb!(0x8A, 0x86, 0x7D),
    border: rgb!(0x4A, 0x46, 0x40),
    border_focus: rgb!(0xD9, 0x77, 0x57),
    success: rgb!(0x87, 0xB3, 0x7B),
    warning: rgb!(0xE0, 0xB4, 0x4C),
    danger: rgb!(0xE0, 0x6C, 0x5F),
    info: rgb!(0x7B, 0xA9, 0xC7),
    highlight_bg: rgb!(0xD9, 0x77, 0x57),
    highlight_fg: rgb!(0x1B, 0x19, 0x16),
};

const CODEX: Theme = Theme {
    id: "codex",
    label: "Codex",
    accent: rgb!(0x37, 0xE0, 0xB0),
    accent_alt: rgb!(0x8A, 0xB4, 0xFF),
    fg: rgb!(0xE3, 0xE8, 0xE6),
    muted: rgb!(0x6B, 0x76, 0x74),
    border: rgb!(0x2A, 0x33, 0x31),
    border_focus: rgb!(0x37, 0xE0, 0xB0),
    success: rgb!(0x37, 0xE0, 0xB0),
    warning: rgb!(0xF2, 0xC9, 0x4C),
    danger: rgb!(0xF2, 0x5F, 0x5F),
    info: rgb!(0x8A, 0xB4, 0xFF),
    highlight_bg: rgb!(0x37, 0xE0, 0xB0),
    highlight_fg: rgb!(0x0A, 0x14, 0x12),
};

const BTOP: Theme = Theme {
    id: "btop",
    label: "Btop",
    accent: rgb!(0x6A, 0xE0, 0x5A),
    accent_alt: rgb!(0xF0, 0xE0, 0x50),
    fg: rgb!(0xD4, 0xE6, 0xD4),
    muted: rgb!(0x5A, 0x6A, 0x5A),
    border: rgb!(0x30, 0x40, 0x30),
    border_focus: rgb!(0x6A, 0xE0, 0x5A),
    success: rgb!(0x6A, 0xE0, 0x5A),
    warning: rgb!(0xF0, 0xE0, 0x50),
    danger: rgb!(0xE0, 0x50, 0x50),
    info: rgb!(0x50, 0xC0, 0xE0),
    highlight_bg: rgb!(0x6A, 0xE0, 0x5A),
    highlight_fg: rgb!(0x0A, 0x14, 0x0A),
};

const NORD: Theme = Theme {
    id: "nord",
    label: "Nord",
    accent: rgb!(0x88, 0xC0, 0xD0),
    accent_alt: rgb!(0x81, 0xA1, 0xC1),
    fg: rgb!(0xE5, 0xE9, 0xF0),
    muted: rgb!(0x61, 0x6E, 0x88),
    border: rgb!(0x3B, 0x42, 0x52),
    border_focus: rgb!(0x88, 0xC0, 0xD0),
    success: rgb!(0xA3, 0xBE, 0x8C),
    warning: rgb!(0xEB, 0xCB, 0x8B),
    danger: rgb!(0xBF, 0x61, 0x6A),
    info: rgb!(0x81, 0xA1, 0xC1),
    highlight_bg: rgb!(0x88, 0xC0, 0xD0),
    highlight_fg: rgb!(0x2E, 0x34, 0x40),
};

const DRACULA: Theme = Theme {
    id: "dracula",
    label: "Dracula",
    accent: rgb!(0xBD, 0x93, 0xF9),
    accent_alt: rgb!(0xFF, 0x79, 0xC6),
    fg: rgb!(0xF8, 0xF8, 0xF2),
    muted: rgb!(0x62, 0x72, 0xA4),
    border: rgb!(0x3A, 0x3D, 0x52),
    border_focus: rgb!(0xBD, 0x93, 0xF9),
    success: rgb!(0x50, 0xFA, 0x7B),
    warning: rgb!(0xF1, 0xFA, 0x8C),
    danger: rgb!(0xFF, 0x55, 0x55),
    info: rgb!(0x8B, 0xE9, 0xFD),
    highlight_bg: rgb!(0xBD, 0x93, 0xF9),
    highlight_fg: rgb!(0x1E, 0x1F, 0x29),
};

const MONO: Theme = Theme {
    id: "mono",
    label: "Mono (TTY-safe)",
    accent: Color::White,
    accent_alt: Color::Gray,
    fg: Color::White,
    muted: Color::DarkGray,
    border: Color::DarkGray,
    border_focus: Color::White,
    success: Color::Green,
    warning: Color::Yellow,
    danger: Color::Red,
    info: Color::Cyan,
    highlight_bg: Color::White,
    highlight_fg: Color::Black,
};

pub const ALL: &[Theme] = &[CLAUDE, CODEX, BTOP, NORD, DRACULA, MONO];

impl Theme {
    pub fn default_theme() -> Theme {
        CLAUDE
    }

    pub fn by_id(id: &str) -> Theme {
        ALL.iter()
            .find(|t| t.id.eq_ignore_ascii_case(id))
            .copied()
            .unwrap_or_else(Theme::default_theme)
    }

    pub fn index(self) -> usize {
        ALL.iter().position(|t| t.id == self.id).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_id_falls_back_to_default_for_unknown_name() {
        assert_eq!(Theme::by_id("does-not-exist"), Theme::default_theme());
    }

    #[test]
    fn by_id_is_case_insensitive() {
        assert_eq!(Theme::by_id("BTOP").id, "btop");
        assert_eq!(Theme::by_id("Nord").id, "nord");
    }

    #[test]
    fn all_themes_have_unique_ids() {
        let mut ids: Vec<&str> = ALL.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ALL.len());
    }

    #[test]
    fn index_matches_position_in_all() {
        for (i, theme) in ALL.iter().enumerate() {
            assert_eq!(theme.index(), i);
        }
    }
}
