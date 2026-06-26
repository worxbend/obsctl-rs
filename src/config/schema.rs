use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

use super::model::Config;

pub struct ValidationWarning(pub String);

pub fn validate(config: &Config) -> Result<Vec<ValidationWarning>> {
    let mut warnings = Vec::new();

    if config.version != 1 {
        return Err(ObsctlError::ConfigInvalid(format!(
            "unsupported config version: {}",
            config.version
        )));
    }

    if config.connection.host.is_empty() {
        return Err(ObsctlError::ConfigInvalid(
            "connection.host must not be blank".to_string(),
        ));
    }

    if config.connection.port == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.port must be in range 1-65535".to_string(),
        ));
    }

    if config.connection.connect_timeout_ms == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.connect_timeout_ms must be positive".to_string(),
        ));
    }

    if config.connection.request_timeout_ms == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.request_timeout_ms must be positive".to_string(),
        ));
    }

    if config.ui.refresh_interval_ms == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "ui.refresh_interval_ms must be positive".to_string(),
        ));
    }

    if config.reconnect.max_delay_ms < config.reconnect.initial_delay_ms {
        return Err(ObsctlError::ConfigInvalid(
            "reconnect.max_delay_ms must be >= initial_delay_ms".to_string(),
        ));
    }

    if config.reconnect.multiplier < 1.0 {
        return Err(ObsctlError::ConfigInvalid(
            "reconnect.multiplier must be >= 1.0".to_string(),
        ));
    }

    if let Some(ref socket_path) = config.server.socket_path
        && socket_path.is_empty()
    {
        return Err(ObsctlError::ConfigInvalid(
            "server.socket_path must not be blank when set".to_string(),
        ));
    }

    if config.connection.password.is_some() {
        warnings.push(ValidationWarning(
            "connection.password contains a plaintext password; use password_env instead"
                .to_string(),
        ));
    }

    if !config.connection.password_env.is_empty()
        && std::env::var(&config.connection.password_env).is_err()
    {
        warnings.push(ValidationWarning(format!(
            "password_env {} is not set; will connect without a password",
            config.connection.password_env
        )));
    }

    validate_no_duplicate_aliases(config)?;

    Ok(warnings)
}

fn validate_no_duplicate_aliases(config: &Config) -> Result<()> {
    let mut scene_aliases = std::collections::HashSet::new();
    let mut scene_shortcuts = std::collections::HashSet::new();

    for scene in &config.scenes {
        if let Some(alias) = &scene.alias
            && !scene_aliases.insert(alias.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate scene alias: {alias}"
            )));
        }
        if let Some(shortcut) = &scene.shortcut
            && !scene_shortcuts.insert(shortcut.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate scene shortcut: {shortcut}"
            )));
        }
    }

    let mut audio_aliases = std::collections::HashSet::new();
    let mut audio_shortcuts = std::collections::HashSet::new();

    for input in &config.audio.inputs {
        if let Some(alias) = &input.alias
            && !audio_aliases.insert(alias.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate audio alias: {alias}"
            )));
        }
        if let Some(shortcut) = &input.shortcut
            && !audio_shortcuts.insert(shortcut.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate audio shortcut: {shortcut}"
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{AudioInputConfig, SceneConfig};

    fn valid_config() -> Config {
        let mut c = Config::default();
        c.connection.password_env = String::new();
        c
    }

    #[test]
    fn valid_default_config() {
        let c = valid_config();
        assert!(validate(&c).is_ok());
    }

    #[test]
    fn rejects_blank_host() {
        let mut c = valid_config();
        c.connection.host = String::new();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_bad_version() {
        let mut c = valid_config();
        c.version = 99;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn duplicate_scene_alias_rejected() {
        let mut c = valid_config();
        c.scenes = vec![
            SceneConfig {
                name: "A".to_string(),
                alias: Some("same".to_string()),
                ..Default::default()
            },
            SceneConfig {
                name: "B".to_string(),
                alias: Some("same".to_string()),
                ..Default::default()
            },
        ];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn plaintext_password_warns_without_leaking() {
        let mut c = valid_config();
        c.connection.password = Some("hunter2".to_string());
        let warnings = validate(&c).unwrap();
        assert!(!warnings.is_empty());
        assert!(!warnings[0].0.contains("hunter2"));
    }

    #[test]
    fn connection_config_debug_redacts_password() {
        use crate::config::model::ConnectionConfig;
        let cfg = ConnectionConfig {
            password: Some("supersecret".to_string()),
            ..ConnectionConfig::default()
        };
        let debug = format!("{cfg:?}");
        assert!(
            !debug.contains("supersecret"),
            "debug must not leak password: {debug}"
        );
        assert!(debug.contains("<redacted>"), "debug should show <redacted>");
    }

    #[test]
    fn connection_config_debug_shows_none_when_no_password() {
        use crate::config::model::ConnectionConfig;
        let cfg = ConnectionConfig::default();
        let debug = format!("{cfg:?}");
        assert!(debug.contains("None"), "no password should show None");
    }

    #[test]
    fn duplicate_audio_alias_rejected() {
        let mut c = valid_config();
        c.audio.inputs = vec![
            AudioInputConfig {
                name: "Mic A".to_string(),
                alias: Some("mic".to_string()),
                ..Default::default()
            },
            AudioInputConfig {
                name: "Mic B".to_string(),
                alias: Some("mic".to_string()),
                ..Default::default()
            },
        ];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_max_delay_less_than_initial() {
        let mut c = valid_config();
        c.reconnect.initial_delay_ms = 5000;
        c.reconnect.max_delay_ms = 100;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_multiplier_less_than_one() {
        let mut c = valid_config();
        c.reconnect.multiplier = 0.5;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn missing_password_env_warns_but_does_not_fail() {
        let mut c = Config::default();
        c.connection.password_env = "OBSCTL_SCHEMA_TEST_NONEXISTENT_VAR_F7D3C2A1B0".to_string();
        let warnings = validate(&c).unwrap();
        assert!(!warnings.is_empty());
        assert!(warnings.iter().any(|w| w.0.contains("not set")));
    }

    #[test]
    fn rejects_blank_socket_path_when_set() {
        let mut c = valid_config();
        c.server.socket_path = Some(String::new());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn duplicate_scene_shortcut_rejected() {
        let mut c = valid_config();
        c.scenes = vec![
            SceneConfig {
                name: "A".to_string(),
                shortcut: Some("s".to_string()),
                ..Default::default()
            },
            SceneConfig {
                name: "B".to_string(),
                shortcut: Some("s".to_string()),
                ..Default::default()
            },
        ];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn duplicate_audio_shortcut_rejected() {
        let mut c = valid_config();
        c.audio.inputs = vec![
            AudioInputConfig {
                name: "Mic A".to_string(),
                shortcut: Some("m".to_string()),
                ..Default::default()
            },
            AudioInputConfig {
                name: "Mic B".to_string(),
                shortcut: Some("m".to_string()),
                ..Default::default()
            },
        ];
        assert!(validate(&c).is_err());
    }
}

#[cfg(test)]
mod loader_tests {
    use crate::config::loader;
    use tempfile::TempDir;

    #[test]
    fn rejects_unknown_top_level_key() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\nconnection:\n  host: 127.0.0.1\nunknown_field: true\n",
        )
        .unwrap();
        let result = loader::load(&path);
        assert!(result.is_err(), "unknown top-level key should be rejected");
    }

    #[test]
    fn legacy_connection_reconnect_is_migrated() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        let yaml = r#"
version: 1
connection:
  host: "127.0.0.1"
  port: 4455
  password_env: ""
  connect_timeout_ms: 3000
  request_timeout_ms: 2500
  reconnect:
    enabled: true
    endless: false
    initial_delay_ms: 1000
    max_delay_ms: 5000
    multiplier: 2.0
    jitter_ms: 100
"#;
        std::fs::write(&path, yaml).unwrap();
        let config = loader::load(&path).expect("should load with legacy reconnect");
        assert!(!config.reconnect.endless);
        assert_eq!(config.reconnect.initial_delay_ms, 1000);
        assert_eq!(config.reconnect.max_delay_ms, 5000);
        assert!((config.reconnect.multiplier - 2.0).abs() < 1e-9);
    }
}
