//! Color themes for the TUI, selectable via `ui.theme` in config or the
//! in-app settings view (styled after btop's theme switcher).

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub id: &'static str,
    pub label: &'static str,
    /// Base terminal background, painted behind the whole UI.
    pub bg: Color,
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
    bg: rgb!(0x1B, 0x19, 0x16),
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
    bg: rgb!(0x0A, 0x14, 0x12),
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
    bg: rgb!(0x0A, 0x14, 0x0A),
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
    bg: rgb!(0x2E, 0x34, 0x40),
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
    bg: rgb!(0x28, 0x2A, 0x36),
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
    // Reset (not a fixed color) so this theme never overrides the user's
    // own terminal background — that's the point of a "TTY-safe" theme.
    bg: Color::Reset,
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

const GRUVBOX: Theme = Theme {
    id: "gruvbox",
    label: "Gruvbox",
    bg: rgb!(0x28, 0x28, 0x28),
    accent: rgb!(0xFE, 0x80, 0x19),
    accent_alt: rgb!(0xD3, 0x86, 0x9B),
    fg: rgb!(0xEB, 0xDB, 0xB2),
    muted: rgb!(0x92, 0x83, 0x74),
    border: rgb!(0x3C, 0x38, 0x36),
    border_focus: rgb!(0xFE, 0x80, 0x19),
    success: rgb!(0xB8, 0xBB, 0x26),
    warning: rgb!(0xFA, 0xBD, 0x2F),
    danger: rgb!(0xFB, 0x49, 0x34),
    info: rgb!(0x83, 0xA5, 0x98),
    highlight_bg: rgb!(0xFE, 0x80, 0x19),
    highlight_fg: rgb!(0x28, 0x28, 0x28),
};

const SOLARIZED_DARK: Theme = Theme {
    id: "solarized-dark",
    label: "Solarized Dark",
    bg: rgb!(0x00, 0x2B, 0x36),
    accent: rgb!(0x26, 0x8B, 0xD2),
    accent_alt: rgb!(0x2A, 0xA1, 0x98),
    fg: rgb!(0x93, 0xA1, 0xA1),
    muted: rgb!(0x58, 0x6E, 0x75),
    border: rgb!(0x07, 0x36, 0x42),
    border_focus: rgb!(0x26, 0x8B, 0xD2),
    success: rgb!(0x85, 0x99, 0x00),
    warning: rgb!(0xB5, 0x89, 0x00),
    danger: rgb!(0xDC, 0x32, 0x2F),
    info: rgb!(0x2A, 0xA1, 0x98),
    highlight_bg: rgb!(0x26, 0x8B, 0xD2),
    highlight_fg: rgb!(0x00, 0x2B, 0x36),
};

const MONOKAI: Theme = Theme {
    id: "monokai",
    label: "Monokai",
    bg: rgb!(0x27, 0x28, 0x22),
    accent: rgb!(0xA6, 0xE2, 0x2E),
    accent_alt: rgb!(0xAE, 0x81, 0xFF),
    fg: rgb!(0xF8, 0xF8, 0xF2),
    muted: rgb!(0x75, 0x71, 0x5E),
    border: rgb!(0x3E, 0x3D, 0x32),
    border_focus: rgb!(0xA6, 0xE2, 0x2E),
    success: rgb!(0xA6, 0xE2, 0x2E),
    warning: rgb!(0xE6, 0xDB, 0x74),
    danger: rgb!(0xF9, 0x26, 0x72),
    info: rgb!(0x66, 0xD9, 0xEF),
    highlight_bg: rgb!(0xA6, 0xE2, 0x2E),
    highlight_fg: rgb!(0x27, 0x28, 0x22),
};

const ONE_DARK: Theme = Theme {
    id: "one-dark",
    label: "One Dark",
    bg: rgb!(0x28, 0x2C, 0x34),
    accent: rgb!(0x61, 0xAF, 0xEF),
    accent_alt: rgb!(0xC6, 0x78, 0xDD),
    fg: rgb!(0xAB, 0xB2, 0xBF),
    muted: rgb!(0x5C, 0x63, 0x70),
    border: rgb!(0x3E, 0x44, 0x51),
    border_focus: rgb!(0x61, 0xAF, 0xEF),
    success: rgb!(0x98, 0xC3, 0x79),
    warning: rgb!(0xE5, 0xC0, 0x7B),
    danger: rgb!(0xE0, 0x6C, 0x75),
    info: rgb!(0x56, 0xB6, 0xC2),
    highlight_bg: rgb!(0x61, 0xAF, 0xEF),
    highlight_fg: rgb!(0x28, 0x2C, 0x34),
};

const TOKYO_NIGHT: Theme = Theme {
    id: "tokyo-night",
    label: "Tokyo Night",
    bg: rgb!(0x1A, 0x1B, 0x26),
    accent: rgb!(0x7A, 0xA2, 0xF7),
    accent_alt: rgb!(0xBB, 0x9A, 0xF7),
    fg: rgb!(0xC0, 0xCA, 0xF5),
    muted: rgb!(0x56, 0x5F, 0x89),
    border: rgb!(0x24, 0x28, 0x3B),
    border_focus: rgb!(0x7A, 0xA2, 0xF7),
    success: rgb!(0x9E, 0xCE, 0x6A),
    warning: rgb!(0xE0, 0xAF, 0x68),
    danger: rgb!(0xF7, 0x76, 0x8E),
    info: rgb!(0x7D, 0xCF, 0xFF),
    highlight_bg: rgb!(0x7A, 0xA2, 0xF7),
    highlight_fg: rgb!(0x1A, 0x1B, 0x26),
};

const CATPPUCCIN_MOCHA: Theme = Theme {
    id: "catppuccin-mocha",
    label: "Catppuccin Mocha",
    bg: rgb!(0x1E, 0x1E, 0x2E),
    accent: rgb!(0xCB, 0xA6, 0xF7),
    accent_alt: rgb!(0x89, 0xB4, 0xFA),
    fg: rgb!(0xCD, 0xD6, 0xF4),
    muted: rgb!(0xA6, 0xAD, 0xC8),
    border: rgb!(0x31, 0x32, 0x44),
    border_focus: rgb!(0xCB, 0xA6, 0xF7),
    success: rgb!(0xA6, 0xE3, 0xA1),
    warning: rgb!(0xF9, 0xE2, 0xAF),
    danger: rgb!(0xF3, 0x8B, 0xA8),
    info: rgb!(0x89, 0xDC, 0xEB),
    highlight_bg: rgb!(0xCB, 0xA6, 0xF7),
    highlight_fg: rgb!(0x1E, 0x1E, 0x2E),
};

const ROSE_PINE: Theme = Theme {
    id: "rose-pine",
    label: "Rose Pine",
    bg: rgb!(0x19, 0x17, 0x24),
    accent: rgb!(0xC4, 0xA7, 0xE7),
    accent_alt: rgb!(0xEB, 0xBC, 0xBA),
    fg: rgb!(0xE0, 0xDE, 0xF4),
    muted: rgb!(0x6E, 0x6A, 0x86),
    border: rgb!(0x26, 0x23, 0x3A),
    border_focus: rgb!(0xC4, 0xA7, 0xE7),
    success: rgb!(0x9C, 0xCF, 0xD8),
    warning: rgb!(0xF6, 0xC1, 0x77),
    danger: rgb!(0xEB, 0x6F, 0x92),
    info: rgb!(0x9C, 0xCF, 0xD8),
    highlight_bg: rgb!(0xC4, 0xA7, 0xE7),
    highlight_fg: rgb!(0x19, 0x17, 0x24),
};

pub const ALL: &[Theme] = &[
    CLAUDE,
    CODEX,
    BTOP,
    NORD,
    DRACULA,
    GRUVBOX,
    SOLARIZED_DARK,
    MONOKAI,
    ONE_DARK,
    TOKYO_NIGHT,
    CATPPUCCIN_MOCHA,
    ROSE_PINE,
    MONO,
];

/// The `ui.theme` id that selects the user-supplied `ui.custom_theme` palette.
pub const CUSTOM_ID: &str = "custom";

/// A user-supplied palette, read from config `ui.custom_theme`. Every field
/// is an optional `"#RRGGBB"` (or `"RRGGBB"`) hex string; unset fields fall
/// back to the corresponding [`Theme::default_theme`] color so a partial
/// override (e.g. just `accent`) still produces a usable theme.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomThemeSpec {
    pub bg: Option<String>,
    pub accent: Option<String>,
    pub accent_alt: Option<String>,
    pub fg: Option<String>,
    pub muted: Option<String>,
    pub border: Option<String>,
    pub border_focus: Option<String>,
    pub success: Option<String>,
    pub warning: Option<String>,
    pub danger: Option<String>,
    pub info: Option<String>,
    pub highlight_bg: Option<String>,
    pub highlight_fg: Option<String>,
}

/// Parse a `"#RRGGBB"` or `"RRGGBB"` hex string into a truecolor `Color`.
/// Returns `None` for anything else (wrong length, non-hex digits, etc.)
/// rather than erroring, since a bad custom color should degrade to the
/// default rather than crash the TUI.
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

impl Theme {
    pub fn default_theme() -> Theme {
        CLAUDE
    }

    pub fn by_id(id: &str) -> Theme {
        if id.eq_ignore_ascii_case("default") {
            return Theme::default_theme();
        }
        ALL.iter()
            .find(|t| t.id.eq_ignore_ascii_case(id))
            .copied()
            .unwrap_or_else(Theme::default_theme)
    }

    /// Resolve the configured theme, honoring the reserved [`CUSTOM_ID`]
    /// which builds a one-off theme from `custom` instead of looking it up
    /// in [`ALL`].
    pub fn resolve(id: &str, custom: Option<&CustomThemeSpec>) -> Theme {
        if id.eq_ignore_ascii_case(CUSTOM_ID) {
            Theme::from_custom_spec(custom.cloned().unwrap_or_default())
        } else {
            Theme::by_id(id)
        }
    }

    pub fn from_custom_spec(spec: CustomThemeSpec) -> Theme {
        let base = Theme::default_theme();
        let pick = |value: &Option<String>, fallback: Color| {
            value.as_deref().and_then(parse_hex).unwrap_or(fallback)
        };
        Theme {
            id: CUSTOM_ID,
            label: "Custom",
            bg: pick(&spec.bg, base.bg),
            accent: pick(&spec.accent, base.accent),
            accent_alt: pick(&spec.accent_alt, base.accent_alt),
            fg: pick(&spec.fg, base.fg),
            muted: pick(&spec.muted, base.muted),
            border: pick(&spec.border, base.border),
            border_focus: pick(&spec.border_focus, base.border_focus),
            success: pick(&spec.success, base.success),
            warning: pick(&spec.warning, base.warning),
            danger: pick(&spec.danger, base.danger),
            info: pick(&spec.info, base.info),
            highlight_bg: pick(&spec.highlight_bg, base.highlight_bg),
            highlight_fg: pick(&spec.highlight_fg, base.highlight_fg),
        }
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

    #[test]
    fn by_id_treats_legacy_default_as_claude() {
        assert_eq!(Theme::by_id("default"), CLAUDE);
    }

    #[test]
    fn parse_hex_accepts_with_and_without_hash() {
        assert_eq!(parse_hex("#ff0080"), Some(Color::Rgb(0xff, 0x00, 0x80)));
        assert_eq!(parse_hex("ff0080"), Some(Color::Rgb(0xff, 0x00, 0x80)));
    }

    #[test]
    fn parse_hex_rejects_malformed_input() {
        assert_eq!(parse_hex("#fff"), None);
        assert_eq!(parse_hex("not-a-color"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn from_custom_spec_uses_overrides_and_falls_back_for_unset_fields() {
        let spec = CustomThemeSpec {
            accent: Some("#112233".to_string()),
            ..Default::default()
        };
        let theme = Theme::from_custom_spec(spec);
        assert_eq!(theme.id, CUSTOM_ID);
        assert_eq!(theme.accent, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(theme.fg, Theme::default_theme().fg);
    }

    #[test]
    fn resolve_dispatches_custom_id_to_custom_spec() {
        let spec = CustomThemeSpec {
            accent: Some("#abcdef".to_string()),
            ..Default::default()
        };
        let theme = Theme::resolve("custom", Some(&spec));
        assert_eq!(theme.id, CUSTOM_ID);
        assert_eq!(theme.accent, Color::Rgb(0xab, 0xcd, 0xef));

        let named = Theme::resolve("nord", Some(&spec));
        assert_eq!(named.id, "nord");
    }

    #[test]
    fn resolve_custom_without_spec_falls_back_to_default_colors() {
        let theme = Theme::resolve("custom", None);
        assert_eq!(theme.id, CUSTOM_ID);
        assert_eq!(theme.accent, Theme::default_theme().accent);
    }

    #[test]
    fn mono_theme_does_not_override_terminal_background() {
        assert_eq!(MONO.bg, Color::Reset);
    }

    #[test]
    fn from_custom_spec_overrides_background() {
        let spec = CustomThemeSpec {
            bg: Some("#101010".to_string()),
            ..Default::default()
        };
        let theme = Theme::from_custom_spec(spec);
        assert_eq!(theme.bg, Color::Rgb(0x10, 0x10, 0x10));
    }

    #[test]
    fn from_custom_spec_falls_back_to_default_background() {
        let theme = Theme::from_custom_spec(CustomThemeSpec::default());
        assert_eq!(theme.bg, Theme::default_theme().bg);
    }
}
