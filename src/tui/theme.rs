//! Color themes for the TUI, selectable via `ui.theme` in config or the
//! in-app settings view (styled after btop's theme switcher).

use ratatui::style::{Color, Modifier, Style};
use rust_i18n::t;

use crate::config::model::CustomThemeConfig;
use crate::tui::anim;

/// How much of the highlight color is mixed into the panel background for the
/// selected row. Low enough that the row still reads as part of the list, high
/// enough to locate the cursor at a glance on every built-in palette.
const SELECTION_TINT_FOCUSED: f32 = 0.32;

/// The same tint for a list whose panel does not hold focus — present so the
/// cursor position is not lost when you tab away, but faint enough that only
/// the focused panel draws the eye.
const SELECTION_TINT_UNFOCUSED: f32 = 0.12;

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

/// One line per built-in RGB theme: id, label, then the palette colors as
/// packed `0xRRGGBB` literals in [`Theme`] field order (bg, accent,
/// accent_alt, fg, muted, border, border_focus, success, warning, danger,
/// info, highlight_bg, highlight_fg). Themes built from named terminal colors
/// instead of RGB (like [`MONO`]) stay plain struct literals.
macro_rules! theme {
    ($id:literal, $label:literal;
     $bg:literal, $accent:literal, $accent_alt:literal, $fg:literal, $muted:literal,
     $border:literal, $border_focus:literal, $success:literal, $warning:literal,
     $danger:literal, $info:literal, $highlight_bg:literal, $highlight_fg:literal $(,)?) => {
        Theme {
            id: $id,
            label: $label,
            bg: rgb!($bg),
            accent: rgb!($accent),
            accent_alt: rgb!($accent_alt),
            fg: rgb!($fg),
            muted: rgb!($muted),
            border: rgb!($border),
            border_focus: rgb!($border_focus),
            success: rgb!($success),
            warning: rgb!($warning),
            danger: rgb!($danger),
            info: rgb!($info),
            highlight_bg: rgb!($highlight_bg),
            highlight_fg: rgb!($highlight_fg),
        }
    };
}

/// A packed `0xRRGGBB` literal as a truecolor `Color`, split into channels at
/// compile time.
macro_rules! rgb {
    ($hex:literal) => {
        Color::Rgb(
            (($hex >> 16) & 0xFF) as u8,
            (($hex >> 8) & 0xFF) as u8,
            ($hex & 0xFF) as u8,
        )
    };
}

const CLAUDE: Theme = theme! { "claude", "Claude"; 0x1B1916, 0xD97757, 0xE8C59E, 0xECE8E1, 0x8A867D, 0x4A4640, 0xD97757, 0x87B37B, 0xE0B44C, 0xE06C5F, 0x7BA9C7, 0xD97757, 0x1B1916 };

const CODEX: Theme = theme! { "codex", "Codex"; 0x0A1412, 0x37E0B0, 0x8AB4FF, 0xE3E8E6, 0x6B7674, 0x2A3331, 0x37E0B0, 0x37E0B0, 0xF2C94C, 0xF25F5F, 0x8AB4FF, 0x37E0B0, 0x0A1412 };

const BTOP: Theme = theme! { "btop", "Btop"; 0x0A140A, 0x6AE05A, 0xF0E050, 0xD4E6D4, 0x5A6A5A, 0x304030, 0x6AE05A, 0x6AE05A, 0xF0E050, 0xE05050, 0x50C0E0, 0x6AE05A, 0x0A140A };

const NORD: Theme = theme! { "nord", "Nord"; 0x2E3440, 0x88C0D0, 0x81A1C1, 0xE5E9F0, 0x616E88, 0x3B4252, 0x88C0D0, 0xA3BE8C, 0xEBCB8B, 0xBF616A, 0x81A1C1, 0x88C0D0, 0x2E3440 };

const DRACULA: Theme = theme! { "dracula", "Dracula"; 0x282A36, 0xBD93F9, 0xFF79C6, 0xF8F8F2, 0x6272A4, 0x3A3D52, 0xBD93F9, 0x50FA7B, 0xF1FA8C, 0xFF5555, 0x8BE9FD, 0xBD93F9, 0x1E1F29 };

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

const GRUVBOX: Theme = theme! { "gruvbox", "Gruvbox"; 0x282828, 0xFE8019, 0xD3869B, 0xEBDBB2, 0x928374, 0x3C3836, 0xFE8019, 0xB8BB26, 0xFABD2F, 0xFB4934, 0x83A598, 0xFE8019, 0x282828 };

const SOLARIZED_DARK: Theme = theme! { "solarized-dark", "Solarized Dark"; 0x002B36, 0x268BD2, 0x2AA198, 0x93A1A1, 0x586E75, 0x073642, 0x268BD2, 0x859900, 0xB58900, 0xDC322F, 0x2AA198, 0x268BD2, 0x002B36 };

const MONOKAI: Theme = theme! { "monokai", "Monokai"; 0x272822, 0xA6E22E, 0xAE81FF, 0xF8F8F2, 0x75715E, 0x3E3D32, 0xA6E22E, 0xA6E22E, 0xE6DB74, 0xF92672, 0x66D9EF, 0xA6E22E, 0x272822 };

const ONE_DARK: Theme = theme! { "one-dark", "One Dark"; 0x282C34, 0x61AFEF, 0xC678DD, 0xABB2BF, 0x5C6370, 0x3E4451, 0x61AFEF, 0x98C379, 0xE5C07B, 0xE06C75, 0x56B6C2, 0x61AFEF, 0x282C34 };

const TOKYO_NIGHT: Theme = theme! { "tokyo-night", "Tokyo Night"; 0x1A1B26, 0x7AA2F7, 0xBB9AF7, 0xC0CAF5, 0x565F89, 0x24283B, 0x7AA2F7, 0x9ECE6A, 0xE0AF68, 0xF7768E, 0x7DCFFF, 0x7AA2F7, 0x1A1B26 };

const CATPPUCCIN_MOCHA: Theme = theme! { "catppuccin-mocha", "Catppuccin Mocha"; 0x1E1E2E, 0xCBA6F7, 0x89B4FA, 0xCDD6F4, 0xA6ADC8, 0x313244, 0xCBA6F7, 0xA6E3A1, 0xF9E2AF, 0xF38BA8, 0x89DCEB, 0xCBA6F7, 0x1E1E2E };

const ROSE_PINE: Theme = theme! { "rose-pine", "Rose Pine"; 0x191724, 0xC4A7E7, 0xEBBCBA, 0xE0DEF4, 0x6E6A86, 0x26233A, 0xC4A7E7, 0x9CCFD8, 0xF6C177, 0xEB6F92, 0x9CCFD8, 0xC4A7E7, 0x191724 };

const KANAGAWA_WAVE: Theme = theme! { "kanagawa-wave", "Kanagawa Wave"; 0x1F1F28, 0x7E9CD8, 0x957FB8, 0xDCD7BA, 0x727169, 0x2A2A37, 0x7E9CD8, 0x98BB6C, 0xE6C384, 0xE82424, 0x7FB4CA, 0x7E9CD8, 0x1F1F28 };

const EVERFOREST_DARK: Theme = theme! { "everforest-dark", "Everforest Dark"; 0x2D353B, 0xA7C080, 0xD699B6, 0xD3C6AA, 0x859289, 0x475258, 0xA7C080, 0xA7C080, 0xDBBC7F, 0xE67E80, 0x7FBBB3, 0xA7C080, 0x2D353B };

const AYU_MIRAGE: Theme = theme! { "ayu-mirage", "Ayu Mirage"; 0x1F2430, 0xFFB454, 0xD2A6FF, 0xCBCCC6, 0x707A8C, 0x343D46, 0xFFB454, 0xBAE67E, 0xFFD580, 0xF28779, 0x5CCFE6, 0xFFB454, 0x1F2430 };

const GITHUB_DARK: Theme = theme! { "github-dark", "GitHub Dark"; 0x0D1117, 0x58A6FF, 0xBC8CFF, 0xC9D1D9, 0x8B949E, 0x30363D, 0x58A6FF, 0x3FB950, 0xD29922, 0xF85149, 0x58A6FF, 0x58A6FF, 0x0D1117 };

const SOLARIZED_LIGHT: Theme = theme! { "solarized-light", "Solarized Light"; 0xFDF6E3, 0x268BD2, 0x2AA198, 0x657B83, 0x93A1A1, 0xEEE8D5, 0x268BD2, 0x859900, 0xB58900, 0xDC322F, 0x2AA198, 0x268BD2, 0xFDF6E3 };

const CATPPUCCIN_LATTE: Theme = theme! { "catppuccin-latte", "Catppuccin Latte"; 0xEFF1F5, 0x8839EF, 0x1E66F5, 0x4C4F69, 0x9CA0B0, 0xCCD0DA, 0x8839EF, 0x40A02B, 0xDF8E1D, 0xD20F39, 0x04A5E5, 0x8839EF, 0xEFF1F5 };

const GITHUB_LIGHT: Theme = theme! { "github-light", "GitHub Light"; 0xFFFFFF, 0x0969DA, 0x8250DF, 0x1F2328, 0x656D76, 0xD0D7DE, 0x0969DA, 0x1A7F37, 0x9A6700, 0xCF222E, 0x0969DA, 0x0969DA, 0xFFFFFF };

const ROSE_PINE_DAWN: Theme = theme! { "rose-pine-dawn", "Rose Pine Dawn"; 0xFAF4ED, 0xD7827E, 0x907AA9, 0x575279, 0x9893A5, 0xF2E9E1, 0xD7827E, 0x56949F, 0xEA9D34, 0xB4637A, 0x286983, 0xD7827E, 0xFAF4ED };

const NIGHT_OWL: Theme = theme! { "night-owl", "Night Owl"; 0x011627, 0x82AAFF, 0xC792EA, 0xD6DEEB, 0x637777, 0x1D3B53, 0x82AAFF, 0x22DA6E, 0xECC48D, 0xEF5350, 0x21C7A8, 0x82AAFF, 0x011627 };

const MATERIAL_OCEAN: Theme = theme! { "material-ocean", "Material Ocean"; 0x0F111A, 0x84FFFF, 0xC792EA, 0x8F93A2, 0x464B5D, 0x1A1C25, 0x84FFFF, 0xC3E88D, 0xFFCB6B, 0xF07178, 0x89DDFF, 0x84FFFF, 0x0F111A };

const HORIZON: Theme = theme! { "horizon", "Horizon"; 0x1C1E26, 0xE95678, 0xB877DB, 0xD5D8DA, 0x6C6F93, 0x2E303E, 0xE95678, 0x29D398, 0xFAB795, 0xEC6A88, 0x26BBD9, 0xE95678, 0x1C1E26 };

const ICEBERG: Theme = theme! { "iceberg", "Iceberg"; 0x161821, 0x84A0C6, 0xA093C7, 0xC6C8D1, 0x6B7089, 0x2E313F, 0x84A0C6, 0xB4BE82, 0xE2A478, 0xE27878, 0x89B8C2, 0x84A0C6, 0x161821 };

const MOONFLY: Theme = theme! { "moonfly", "Moonfly"; 0x080808, 0x80A0FF, 0xCF87E8, 0xBDBDBD, 0x808080, 0x323437, 0x80A0FF, 0x8CC85F, 0xE3C78A, 0xFF5189, 0x79DAC8, 0x80A0FF, 0x080808 };

const SYNTHWAVE_84: Theme = theme! { "synthwave-84", "Synthwave '84"; 0x262335, 0xFF7EDB, 0x36F9F6, 0xFFFFFF, 0x848BBD, 0x495495, 0xFF7EDB, 0x72F1B8, 0xFEDE5D, 0xFE4450, 0x03EDF9, 0xFF7EDB, 0x262335 };

const MATRIX: Theme = theme! { "matrix", "Matrix"; 0x050B05, 0x00FF41, 0x00CC33, 0xD8FFD8, 0x397D39, 0x0F3D0F, 0x00FF41, 0x00FF41, 0xCFFF04, 0xFF3B30, 0x39FF14, 0x00FF41, 0x050B05 };

const ZENBURN: Theme = theme! { "zenburn", "Zenburn"; 0x3F3F3F, 0xDCA3A3, 0x8CD0D3, 0xDCDCCC, 0x7F9F7F, 0x5F5F5F, 0xDCA3A3, 0x7F9F7F, 0xF0DFAF, 0xCC9393, 0x8CD0D3, 0xDCA3A3, 0x3F3F3F };

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
    KANAGAWA_WAVE,
    EVERFOREST_DARK,
    AYU_MIRAGE,
    GITHUB_DARK,
    SOLARIZED_LIGHT,
    CATPPUCCIN_LATTE,
    GITHUB_LIGHT,
    ROSE_PINE_DAWN,
    NIGHT_OWL,
    MATERIAL_OCEAN,
    HORIZON,
    ICEBERG,
    MOONFLY,
    SYNTHWAVE_84,
    MATRIX,
    ZENBURN,
    MONO,
];

/// Theme at `index` in [`ALL`], clamped to the last entry for anything past
/// the end — a cursor cannot pick a theme that does not exist.
pub fn at(index: usize) -> Theme {
    ALL[index.min(ALL.len() - 1)]
}

/// The `ui.theme` id that selects the user-supplied `ui.custom_theme` palette.
pub const CUSTOM_ID: &str = "custom";

/// The `ui.theme` id of the TTY-safe monochrome palette.
pub const MONO_ID: &str = "mono";

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
    pub fn resolve(id: &str, custom: Option<&CustomThemeConfig>) -> Theme {
        if id.eq_ignore_ascii_case(CUSTOM_ID) {
            let fallback = CustomThemeConfig::default();
            Theme::from_custom_spec(custom.unwrap_or(&fallback))
        } else {
            Theme::by_id(id)
        }
    }

    /// Build a one-off theme from the on-disk `ui.custom_theme` palette. Every
    /// field is an optional `"#RRGGBB"` (or `"RRGGBB"`) hex string; unset or
    /// unparseable fields fall back to the corresponding
    /// [`Theme::default_theme`] color, so a partial override (e.g. just
    /// `accent`) still produces a usable theme.
    pub fn from_custom_spec(spec: &CustomThemeConfig) -> Theme {
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

    /// The theme's name as shown to the user.
    ///
    /// Most themes are named after the palette they copy (Nord, Dracula,
    /// Gruvbox); those are proper nouns and stay as they are in every locale.
    /// The two that are descriptions rather than names — the TTY-safe mono
    /// theme and the user's own custom palette — are translated.
    pub fn display_label(&self) -> std::borrow::Cow<'static, str> {
        match self.id {
            MONO_ID => std::borrow::Cow::Owned(t!("tui.theme.mono").into_owned()),
            CUSTOM_ID => std::borrow::Cow::Owned(t!("tui.theme.custom").into_owned()),
            _ => std::borrow::Cow::Borrowed(self.label),
        }
    }

    pub fn index(self) -> usize {
        ALL.iter().position(|t| t.id == self.id).unwrap_or(0)
    }

    /// Row style for the selected entry of a dashboard list.
    ///
    /// Terminals have no alpha channel, so "semi-transparent" is approximated
    /// by mixing the theme's highlight color into its own background: the
    /// selected row reads as a tint of the panel rather than a solid bar
    /// stamped over it. Because the style sets only a background, the row's
    /// spans keep their own colors (scene markers, aliases, shortcuts) instead
    /// of being flattened to one foreground — which is what makes it look
    /// translucent rather than painted over.
    ///
    /// Every theme derives its own tint from its own palette, so this stays
    /// correct for the built-ins and for user-defined custom themes alike.
    pub fn selection_style(self, focused: bool) -> Style {
        let modifier = if focused {
            Modifier::BOLD
        } else {
            Modifier::DIM
        };
        let tint = if focused {
            SELECTION_TINT_FOCUSED
        } else {
            SELECTION_TINT_UNFOCUSED
        };

        match (self.bg, self.highlight_bg) {
            (Color::Rgb(..), Color::Rgb(..)) => Style::default()
                .bg(anim::blend(self.bg, self.highlight_bg, tint))
                .add_modifier(modifier),
            // Named terminal colors have no channels to mix, and `mono` uses
            // `Color::Reset` for its background precisely so it inherits the
            // user's terminal. There is nothing to blend against, so fall back
            // to dimming: still softer than the old solid bar, and it survives
            // on a 16-color TTY.
            _ if focused => Style::default()
                .bg(self.highlight_bg)
                .fg(self.highlight_fg)
                .add_modifier(Modifier::DIM),
            _ => Style::default().add_modifier(Modifier::DIM),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Brand palettes keep their own name in every locale; the two themes
    /// whose "name" is really a description go through the locale files.
    #[test]
    fn display_label_translates_only_the_descriptive_names() {
        assert_eq!(Theme::by_id("nord").display_label(), "Nord");
        assert_eq!(Theme::by_id(MONO_ID).display_label(), "Mono (TTY-safe)");
        assert_eq!(
            Theme::from_custom_spec(&CustomThemeConfig::default()).display_label(),
            "Custom"
        );
    }

    #[test]
    fn by_id_falls_back_to_default_for_unknown_name() {
        assert_eq!(Theme::by_id("does-not-exist"), Theme::default_theme());
    }

    #[test]
    fn by_id_is_case_insensitive() {
        assert_eq!(Theme::by_id("BTOP").id, "btop");
        assert_eq!(Theme::by_id("Nord").id, "nord");
        assert_eq!(Theme::by_id("Kanagawa-Wave").id, "kanagawa-wave");
    }

    #[test]
    fn light_themes_use_dark_text_on_light_backgrounds() {
        assert_eq!(SOLARIZED_LIGHT.highlight_fg, SOLARIZED_LIGHT.bg);
        assert_eq!(CATPPUCCIN_LATTE.highlight_fg, CATPPUCCIN_LATTE.bg);
        assert_eq!(GITHUB_LIGHT.highlight_fg, GITHUB_LIGHT.bg);
        assert_eq!(ROSE_PINE_DAWN.highlight_fg, ROSE_PINE_DAWN.bg);
        assert_ne!(SOLARIZED_LIGHT.fg, SOLARIZED_LIGHT.bg);
        assert_ne!(CATPPUCCIN_LATTE.fg, CATPPUCCIN_LATTE.bg);
        assert_ne!(GITHUB_LIGHT.fg, GITHUB_LIGHT.bg);
        assert_ne!(ROSE_PINE_DAWN.fg, ROSE_PINE_DAWN.bg);
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
        let spec = CustomThemeConfig {
            accent: Some("#112233".to_string()),
            ..Default::default()
        };
        let theme = Theme::from_custom_spec(&spec);
        assert_eq!(theme.id, CUSTOM_ID);
        assert_eq!(theme.accent, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(theme.fg, Theme::default_theme().fg);
    }

    #[test]
    fn resolve_dispatches_custom_id_to_custom_spec() {
        let spec = CustomThemeConfig {
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
        let spec = CustomThemeConfig {
            bg: Some("#101010".to_string()),
            ..Default::default()
        };
        let theme = Theme::from_custom_spec(&spec);
        assert_eq!(theme.bg, Color::Rgb(0x10, 0x10, 0x10));
    }

    #[test]
    fn from_custom_spec_falls_back_to_default_background() {
        let theme = Theme::from_custom_spec(&CustomThemeConfig::default());
        assert_eq!(theme.bg, Theme::default_theme().bg);
    }

    fn channels(color: Color) -> (i32, i32, i32) {
        match color {
            Color::Rgb(r, g, b) => (r as i32, g as i32, b as i32),
            other => panic!("expected an RGB color, got {other:?}"),
        }
    }

    /// Largest per-channel distance between two colors.
    fn distance(a: Color, b: Color) -> i32 {
        let (ar, ag, ab) = channels(a);
        let (br, bg, bb) = channels(b);
        (ar - br).abs().max((ag - bg).abs()).max((ab - bb).abs())
    }

    #[test]
    fn selection_sets_only_a_background_so_row_colors_survive() {
        // This is what makes the selection read as translucent: leaving `fg`
        // unset means ratatui does not flatten the row's spans (scene marker,
        // alias, shortcut) to a single foreground.
        let style = Theme::default_theme().selection_style(true);
        assert!(style.bg.is_some(), "selection needs a background tint");
        assert_eq!(
            style.fg, None,
            "setting a foreground would repaint every span in the row"
        );
    }

    #[test]
    fn every_rgb_theme_tints_its_selection_between_background_and_highlight() {
        for theme in ALL.iter().filter(|t| !matches!(t.bg, Color::Reset)) {
            let selection = theme
                .selection_style(true)
                .bg
                .unwrap_or_else(|| panic!("{} has no selection tint", theme.id));

            // A real blend: neither the flat background nor the solid
            // highlight bar it replaces.
            assert_ne!(
                selection, theme.bg,
                "{} selection is invisible against its background",
                theme.id
            );
            assert_ne!(
                selection, theme.highlight_bg,
                "{} selection is still the solid highlight",
                theme.id
            );

            // Each channel lands between the two endpoints.
            let (sr, sg, sb) = channels(selection);
            let (br, bg, bb) = channels(theme.bg);
            let (hr, hg, hb) = channels(theme.highlight_bg);
            for (s, b, h) in [(sr, br, hr), (sg, bg, hg), (sb, bb, hb)] {
                assert!(
                    s >= b.min(h) && s <= b.max(h),
                    "{} selection channel {s} is outside [{b}, {h}]",
                    theme.id
                );
            }
        }
    }

    #[test]
    fn every_rgb_theme_keeps_its_selection_visible_but_not_shouting() {
        // Guards both failure modes at once: a tint too faint to locate the
        // cursor, and one so strong it is the old solid bar by another name.
        for theme in ALL.iter().filter(|t| !matches!(t.bg, Color::Reset)) {
            let selection = theme.selection_style(true).bg.unwrap();
            let from_bg = distance(selection, theme.bg);
            let from_highlight = distance(selection, theme.highlight_bg);
            assert!(
                from_bg >= 12,
                "{} selection is only {from_bg} from its background — too faint to see",
                theme.id
            );
            assert!(
                from_highlight > from_bg,
                "{} selection sits closer to the solid highlight than to the panel",
                theme.id
            );
        }
    }

    #[test]
    fn unfocused_selection_is_fainter_than_focused() {
        for theme in ALL.iter().filter(|t| !matches!(t.bg, Color::Reset)) {
            let focused = theme.selection_style(true).bg.unwrap();
            let unfocused = theme.selection_style(false).bg.unwrap();
            assert!(
                distance(unfocused, theme.bg) < distance(focused, theme.bg),
                "{} does not fade its selection when the panel loses focus",
                theme.id
            );
        }
    }

    #[test]
    fn focused_and_unfocused_selections_are_dimmed_or_bold_respectively() {
        let theme = Theme::default_theme();
        assert!(
            theme
                .selection_style(true)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            theme
                .selection_style(false)
                .add_modifier
                .contains(Modifier::DIM)
        );
    }

    #[test]
    fn mono_theme_dims_instead_of_blending() {
        // `Color::Reset` has no channels to mix, so the TTY-safe theme falls
        // back to dimming rather than producing an invisible selection.
        let focused = MONO.selection_style(true);
        assert_eq!(focused.bg, Some(MONO.highlight_bg));
        assert!(focused.add_modifier.contains(Modifier::DIM));

        let unfocused = MONO.selection_style(false);
        assert_eq!(unfocused.bg, None);
        assert!(unfocused.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn custom_themes_derive_a_tint_from_their_own_palette() {
        let theme = Theme::from_custom_spec(&CustomThemeConfig {
            bg: Some("#000000".to_string()),
            highlight_bg: Some("#ffffff".to_string()),
            ..Default::default()
        });
        let selection = theme.selection_style(true).bg.unwrap();
        // 32% of the way from black to white.
        assert_eq!(selection, Color::Rgb(82, 82, 82));
    }
}
