use serde::{Deserialize, Serialize};

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
            audio: AudioConfig::default(),
            keymap: KeymapConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ServerConfig {
    pub socket_path: Option<String>,
    pub pid_file: Option<String>,
    #[serde(default)]
    pub allow_remote_shutdown: bool,
    #[serde(default = "default_true")]
    pub start_embedded_if_missing: bool,
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
    pub command_palette_prefix: String,
    #[serde(default = "default_true")]
    pub show_icons: bool,
    /// Built-in theme id (see `obsctl tui` settings view), or `"custom"` to
    /// use the palette defined in `custom_theme`.
    pub theme: String,
    #[serde(default)]
    pub custom_theme: Option<CustomThemeConfig>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            refresh_interval_ms: 250,
            command_palette_prefix: "/".to_string(),
            show_icons: true,
            theme: "default".to_string(),
            custom_theme: None,
        }
    }
}

/// User-supplied palette for `ui.theme: custom`. Every color is an optional
/// `"#RRGGBB"` hex string; unset fields fall back to the default theme's
/// colors. See README.md "Custom Themes" for the full field reference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomThemeConfig {
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
