use serde::{Deserialize, Serialize};

use crate::domain::names::normalized_name;
use crate::domain::scene_profiles::{ActiveSceneProfile, SceneVisibility};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub connection: ConnectionConfig,
    #[serde(default)]
    pub reconnect: ReconnectConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub scenes: Vec<SceneConfig>,
    /// Named sets of scene-visibility choices. Declared next to `scenes`
    /// because that is the list they describe; declaration order is also the
    /// order the config file is written back in.
    #[serde(default)]
    pub scene_profiles: Vec<SceneProfileConfig>,
    /// Which of `scene_profiles` is switched on, if any. Matched
    /// case-insensitively; a name nothing answers to is a validation warning,
    /// not an error, and leaves the per-scene `hidden` flags in charge.
    #[serde(default)]
    pub active_scene_profile: Option<String>,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub keymap: KeymapConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            server: ServerConfig::default(),
            connection: ConnectionConfig::default(),
            reconnect: ReconnectConfig::default(),
            ui: UiConfig::default(),
            scenes: Vec::new(),
            scene_profiles: Vec::new(),
            active_scene_profile: None,
            audio: AudioConfig::default(),
            keymap: KeymapConfig::default(),
        }
    }
}

impl Config {
    /// The profile the `active_scene_profile` field names, matched
    /// case-insensitively.
    ///
    /// `None` covers both "no profile selected" and "the selected one is not
    /// defined". Those are the same situation for every caller: fall back to
    /// the per-scene `hidden` flags. Validation is what tells the user about
    /// the second case, so nothing here needs to distinguish them.
    pub fn active_scene_profile(&self) -> Option<&SceneProfileConfig> {
        let wanted = normalized_name(self.active_scene_profile.as_deref()?).ok()?;
        self.scene_profiles
            .iter()
            .find(|profile| normalized_name(&profile.name).is_ok_and(|name| name == wanted))
    }

    /// Which scenes this config hides right now.
    ///
    /// The one call every consumer makes — the daemon when it builds a
    /// snapshot, and anything else that has to know. Re-deriving the rule from
    /// `scenes[].hidden` and `scene_profiles` at a call site is how a caller
    /// ends up disagreeing with the rest of obsctl about what is on screen.
    pub fn scene_visibility(&self) -> SceneVisibility {
        let active = match self.active_scene_profile() {
            Some(profile) => ActiveSceneProfile::Named(&profile.hidden),
            None => ActiveSceneProfile::None,
        };
        SceneVisibility::resolve(
            self.scenes
                .iter()
                .map(|scene| (scene.name.as_str(), scene.hidden)),
            active,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ServerConfig {
    pub socket_path: Option<String>,
    #[serde(default)]
    pub allow_remote_shutdown: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub password_env: String,
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub password: Option<String>,
    /// Legacy field: `connection.reconnect` — migrated to top-level `reconnect` on load.
    #[serde(default, skip_serializing)]
    pub reconnect: Option<ReconnectConfig>,
}

impl std::fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("password_env", &self.password_env)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 4455,
            password_env: "OBS_WEBSOCKET_PASSWORD".to_string(),
            connect_timeout_ms: 3000,
            request_timeout_ms: 2500,
            password: None,
            reconnect: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReconnectConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub endless: bool,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub multiplier: f64,
    pub jitter_ms: u64,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            endless: true,
            initial_delay_ms: 500,
            max_delay_ms: 10000,
            multiplier: 1.8,
            jitter_ms: 250,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub refresh_interval_ms: u64,
    /// Character the TUI command line opens with. `":"` (vim's command
    /// prompt) or `"/"`; both keys always work regardless of this setting,
    /// which only decides what `<leader>` mappings and mouse clicks insert.
    pub command_palette_prefix: String,
    /// Whether the TUI puts the terminal into mouse-reporting mode. Turning
    /// this off gives back the terminal's own click-to-select and
    /// copy-on-drag, at the cost of click/scroll navigation.
    #[serde(default = "default_true")]
    pub mouse: bool,
    /// Enables gradients, Unicode meters, rich borders, and animated terminal
    /// art. Disable for legacy terminals that only render basic ASCII safely.
    #[serde(default = "default_true")]
    pub advanced_ui: bool,
    #[serde(default = "default_true")]
    pub show_icons: bool,
    /// Built-in theme id (see `obsctl tui` settings view), or `"custom"` to
    /// use the palette defined in `custom_theme`.
    pub theme: String,
    #[serde(default)]
    pub custom_theme: Option<CustomThemeConfig>,
    /// UI language: `"en"` or `"uk"`. Unset (or unsupported) falls back to
    /// English. Overridable via the `OBSCTL_LOCALE` env var. See README.md
    /// "Localization" for how to add or override translations.
    #[serde(default)]
    pub locale: Option<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            refresh_interval_ms: 250,
            command_palette_prefix: ":".to_string(),
            mouse: true,
            advanced_ui: true,
            show_icons: true,
            theme: "default".to_string(),
            custom_theme: None,
            locale: None,
        }
    }
}

/// User-supplied palette for `ui.theme: custom`. Every color is an optional
/// `"#RRGGBB"` hex string; unset fields fall back to the default theme's
/// colors. See README.md "Custom Themes" for the full field reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeConfig {
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SceneConfig {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
    pub group: Option<String>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub hidden: bool,
}

/// One named set of scene-visibility choices — an "obsctl scene profile",
/// unrelated to an OBS profile. `hidden` names the scenes this profile hides,
/// spelled as OBS spells them; every other scene is visible while it is active.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SceneProfileConfig {
    pub name: String,
    #[serde(default)]
    pub hidden: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioConfig {
    #[serde(default)]
    pub inputs: Vec<AudioInputConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioInputConfig {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapConfig {
    pub quit: Vec<String>,
    pub command_palette: Vec<String>,
    pub reload_config: Vec<String>,
    pub dump_config: Vec<String>,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            quit: vec!["q".to_string(), "ctrl+c".to_string()],
            command_palette: vec!["/".to_string(), ":".to_string()],
            reload_config: vec!["r".to_string()],
            dump_config: vec!["D".to_string()],
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_ui_is_enabled_by_default() {
        assert!(Config::default().ui.advanced_ui);
        let config: Config = serde_yaml::from_str("version: 1\nui: {}\n").unwrap();
        assert!(config.ui.advanced_ui);
    }

    #[test]
    fn advanced_ui_can_be_disabled_from_yaml() {
        let config: Config =
            serde_yaml::from_str("version: 1\nui:\n  advanced_ui: false\n").unwrap();
        assert!(!config.ui.advanced_ui);
        assert!(config.ui.show_icons);
    }

    /// Scene profiles were added without bumping `version`, so a config
    /// written before they existed has to keep loading untouched.
    #[test]
    fn a_config_without_scene_profiles_still_loads() {
        let config: Config = serde_yaml::from_str("version: 1\n").unwrap();
        assert!(config.scene_profiles.is_empty());
        assert!(config.active_scene_profile.is_none());
    }

    #[test]
    fn scene_profiles_round_trip_through_yaml() {
        let yaml = "version: 1\nscene_profiles:\n  - name: streaming\n    hidden:\n      - Utility BG\nactive_scene_profile: streaming\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.scene_profiles.len(), 1);
        assert_eq!(config.scene_profiles[0].name, "streaming");
        assert_eq!(config.scene_profiles[0].hidden, vec!["Utility BG"]);

        let written = serde_yaml::to_string(&config).unwrap();
        let reloaded: Config = serde_yaml::from_str(&written).unwrap();
        assert_eq!(reloaded.scene_profiles, config.scene_profiles);
        assert_eq!(reloaded.active_scene_profile, config.active_scene_profile);
    }

    fn config_with_profiles(active: Option<&str>) -> Config {
        Config {
            scenes: vec![
                SceneConfig {
                    name: "Main".to_string(),
                    ..Default::default()
                },
                SceneConfig {
                    name: "Utility BG".to_string(),
                    hidden: true,
                    ..Default::default()
                },
            ],
            scene_profiles: vec![SceneProfileConfig {
                name: "Streaming".to_string(),
                hidden: vec!["Main".to_string()],
            }],
            active_scene_profile: active.map(str::to_string),
            ..Config::default()
        }
    }

    #[test]
    fn the_active_scene_profile_is_found_case_insensitively() {
        let config = config_with_profiles(Some(" STREAMING "));
        assert_eq!(
            config.active_scene_profile().map(|p| p.name.as_str()),
            Some("Streaming")
        );
    }

    #[test]
    fn scene_visibility_uses_the_active_profile_instead_of_the_flags() {
        let visibility = config_with_profiles(Some("streaming")).scene_visibility();
        assert!(visibility.is_hidden("Main"));
        assert!(!visibility.is_hidden("Utility BG"));
    }

    #[test]
    fn scene_visibility_falls_back_to_the_flags_without_a_profile() {
        for active in [None, Some("no such profile")] {
            let visibility = config_with_profiles(active).scene_visibility();
            assert!(visibility.is_hidden("Utility BG"), "active: {active:?}");
            assert!(!visibility.is_hidden("Main"), "active: {active:?}");
        }
    }
}
