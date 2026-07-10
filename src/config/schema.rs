use crate::domain::aliases::normalize_alias_or_shortcut;
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::ipc::socket_path::resolve_server_socket_path;
use crate::support::validation::{
    MAX_TARGET_TOKEN_LENGTH, password_config_error_message, resolve_connection_password,
    trim_and_validate_token_with_max_len, validate_no_control_or_whitespace,
};

use super::model::{Config, ConnectionConfig};

#[derive(Debug)]
pub struct ValidationWarning(pub String);

pub fn validate(config: &Config) -> Result<Vec<ValidationWarning>> {
    let mut warnings = Vec::new();

    if config.version != 1 {
        return Err(ObsctlError::ConfigInvalid(format!(
            "unsupported config version: {}",
            config.version
        )));
    }

    validate_connection_config(&config.connection)?;

    validate_resource_names(config)?;

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

    resolve_server_socket_path(config.server.socket_path.as_deref())
        .map(|_| ())
        .map_err(|error| ObsctlError::ConfigInvalid(format!("server.socket_path {error}")))?;

    let resolved_password = resolve_connection_password(
        config.connection.password.as_deref(),
        &config.connection.password_env,
    )
    .map_err(|error| {
        ObsctlError::ConfigInvalid(password_config_error_message(&error).to_string())
    })?;

    if config.connection.password.is_some() {
        warnings.push(ValidationWarning(
            "connection.password contains a plaintext password; use password_env instead"
                .to_string(),
        ));
    }

    if !config.connection.password_env.is_empty()
        && config.connection.password.is_none()
        && resolved_password.is_none()
    {
        warnings.push(ValidationWarning(format!(
            "password_env {} is not set; will connect without a password",
            config.connection.password_env
        )));
    }

    validate_no_duplicate_aliases(config)?;

    Ok(warnings)
}

pub(crate) fn validate_connection_config(config: &ConnectionConfig) -> Result<()> {
    validate_connection_host(&config.host)?;

    if config.port == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.port must be in range 1-65535".to_string(),
        ));
    }

    if config.connect_timeout_ms == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.connect_timeout_ms must be positive".to_string(),
        ));
    }

    if config.request_timeout_ms == 0 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.request_timeout_ms must be positive".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn validate_connection_host(host: &str) -> Result<()> {
    validate_no_control_or_whitespace(host).map_err(|_| {
        ObsctlError::ConfigInvalid(
            "connection.host must not contain control or whitespace characters".to_string(),
        )
    })?;

    if host.contains('/') || host.contains('\\') || host.contains(':') || host.contains('@') {
        return Err(ObsctlError::ConfigInvalid(
            "connection.host must not contain path, separator, userinfo, or colon characters"
                .to_string(),
        ));
    }

    if host.is_empty() {
        return Err(ObsctlError::ConfigInvalid(
            "connection.host must not be blank".to_string(),
        ));
    }

    if host.len() > 253 {
        return Err(ObsctlError::ConfigInvalid(
            "connection.host exceeds DNS-typical length limit".to_string(),
        ));
    }

    Ok(())
}

fn check_unique_aliases_shortcuts<'a>(
    kind: &str,
    items: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
) -> Result<()> {
    let mut aliases = std::collections::HashSet::new();
    let mut shortcuts = std::collections::HashSet::new();

    for (alias, shortcut) in items {
        if let Some(a) = alias {
            let normalized = normalize_alias_or_shortcut(a, kind)?;
            if !aliases.insert(normalized) {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "duplicate {kind} alias: {a}"
                )));
            }
        }
        if let Some(s) = shortcut {
            let normalized = normalize_alias_or_shortcut(s, kind)?;
            if !shortcuts.insert(normalized) {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "duplicate {kind} shortcut: {s}"
                )));
            }
        }
    }

    Ok(())
}

fn validate_no_duplicate_aliases(config: &Config) -> Result<()> {
    check_unique_aliases_shortcuts(
        "scene",
        config
            .scenes
            .iter()
            .map(|s| (s.alias.as_deref(), s.shortcut.as_deref())),
    )?;
    check_unique_aliases_shortcuts(
        "audio",
        config
            .audio
            .inputs
            .iter()
            .map(|i| (i.alias.as_deref(), i.shortcut.as_deref())),
    )?;
    Ok(())
}

fn validate_resource_names(config: &Config) -> Result<()> {
    for scene in &config.scenes {
        trim_and_validate_token_with_max_len(&scene.name, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| ObsctlError::ConfigInvalid(format!("scene name {error}")))?;
    }

    for audio in &config.audio.inputs {
        trim_and_validate_token_with_max_len(&audio.name, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| ObsctlError::ConfigInvalid(format!("audio name {error}")))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{AudioInputConfig, SceneConfig};
    use crate::support::validation::MAX_PASSWORD_LENGTH;
    use tempfile::TempDir;

    fn with_env_var<R>(name: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os(name);
        if let Some(value) = value {
            unsafe { std::env::set_var(name, value) };
        } else {
            unsafe { std::env::remove_var(name) };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var(name, previous) },
            None => unsafe { std::env::remove_var(name) },
        }

        result
    }

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
    fn rejects_whitespace_only_host() {
        let mut c = valid_config();
        c.connection.host = "   ".to_string();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_host_with_control_characters() {
        let mut c = valid_config();
        c.connection.host = "127.0.0.1\n".to_string();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_host_with_userinfo_delimiter() {
        let mut c = valid_config();
        c.connection.host = "user@localhost".to_string();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_overly_long_host_name() {
        let mut c = valid_config();
        c.connection.host = "a".repeat(254);
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_bad_version() {
        let mut c = valid_config();
        c.version = 99;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_zero_connection_port() {
        let mut c = valid_config();
        c.connection.port = 0;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_zero_connection_connect_timeout() {
        let mut c = valid_config();
        c.connection.connect_timeout_ms = 0;
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_zero_connection_request_timeout() {
        let mut c = valid_config();
        c.connection.request_timeout_ms = 0;
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
    fn duplicate_scene_alias_rejected_case_insensitive() {
        let mut c = valid_config();
        c.scenes = vec![
            SceneConfig {
                name: "A".to_string(),
                alias: Some("Same".to_string()),
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
    fn scene_alias_rejects_control_characters() {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "A".to_string(),
            alias: Some("main\ncam".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn scene_shortcut_rejects_control_characters() {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "A".to_string(),
            shortcut: Some("m\0".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn scene_name_rejects_control_characters() {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "Main\nScene".to_string(),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn scene_name_rejects_excessive_length() {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "a".repeat(super::MAX_TARGET_TOKEN_LENGTH + 1),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn blank_scene_alias_rejected() {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "A".to_string(),
            alias: Some("   ".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn blank_scene_shortcut_rejected() {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "A".to_string(),
            shortcut: Some(" \n ".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn audio_alias_rejects_control_characters() {
        let mut c = valid_config();
        c.audio.inputs = vec![AudioInputConfig {
            name: "A".to_string(),
            alias: Some("mic\n".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn audio_name_rejects_control_characters() {
        let mut c = valid_config();
        c.audio.inputs = vec![AudioInputConfig {
            name: "Mic\n1".to_string(),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn audio_name_rejects_excessive_length() {
        let mut c = valid_config();
        c.audio.inputs = vec![AudioInputConfig {
            name: "a".repeat(super::MAX_TARGET_TOKEN_LENGTH + 1),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn audio_shortcut_rejects_control_characters() {
        let mut c = valid_config();
        c.audio.inputs = vec![AudioInputConfig {
            name: "A".to_string(),
            shortcut: Some(" \t ".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn duplicate_scene_shortcut_rejected_case_insensitive() {
        let mut c = valid_config();
        c.scenes = vec![
            SceneConfig {
                name: "A".to_string(),
                shortcut: Some("S".to_string()),
                ..Default::default()
            },
            SceneConfig {
                name: "B".to_string(),
                shortcut: Some(" s ".to_string()),
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
    fn rejects_control_characters_in_password_env() {
        let mut c = valid_config();
        c.connection.password_env = "OBS_TOKEN\n".to_string();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_control_characters_in_password_env_value() {
        with_env_var("OBSCTL_SCHEMA_TEST_PASSWORD_CTRL", Some("abc\t123"), || {
            let mut c = valid_config();
            c.connection.password_env = "OBSCTL_SCHEMA_TEST_PASSWORD_CTRL".to_string();
            let err = validate(&c).unwrap_err();
            assert!(err.to_string().contains("control characters"));
        });
    }

    #[test]
    fn rejects_both_plaintext_password_and_password_env() {
        let mut c = valid_config();
        c.connection.password = Some("secret".to_string());
        c.connection.password_env = "OBS_WEBSOCKET_PASSWORD".to_string();
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_control_characters_in_plaintext_password() {
        let mut c = valid_config();
        c.connection.password = Some("abc\t123".to_string());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_excessive_plaintext_password() {
        let mut c = valid_config();
        c.connection.password = Some("a".repeat(MAX_PASSWORD_LENGTH + 1));
        let err = validate(&c).unwrap_err();
        assert!(
            err.to_string()
                .contains("connection.password exceeds maximum length")
        );
    }

    #[test]
    fn rejects_blank_socket_path_when_set() {
        let mut c = valid_config();
        c.server.socket_path = Some(String::new());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_whitespace_socket_path_when_set() {
        let mut c = valid_config();
        c.server.socket_path = Some(" \t\n ".to_string());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_socket_path_with_wrapped_whitespace() {
        let mut c = valid_config();
        c.server.socket_path = Some(" /tmp/obsctl.sock ".to_string());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_socket_path_with_control_characters() {
        let mut c = valid_config();
        c.server.socket_path = Some("/tmp/obsctl\n.sock".to_string());
        assert!(validate(&c).is_err());
    }

    #[test]
    fn rejects_relative_socket_path() {
        let mut c = valid_config();
        c.server.socket_path = Some("relative.sock".to_string());
        assert!(validate(&c).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_socket_path_with_symlink_parent() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let real_parent = dir.path().join("real");
        std::fs::create_dir(&real_parent).unwrap();
        let link_parent = dir.path().join("link");
        symlink(&real_parent, &link_parent).unwrap();

        let mut c = valid_config();
        c.server.socket_path = Some(link_parent.join("obsctl.sock").to_string_lossy().into());
        assert!(validate(&c).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_socket_path_with_world_writable_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("unsafe-parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();

        let mut c = valid_config();
        c.server.socket_path = Some(parent.join("obsctl.sock").to_string_lossy().into());
        let err = validate(&c).unwrap_err();
        assert!(err.to_string().contains("server.socket_path"));
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

    #[test]
    fn duplicate_audio_alias_rejected_case_insensitive() {
        let mut c = valid_config();
        c.audio.inputs = vec![
            AudioInputConfig {
                name: "Mic A".to_string(),
                alias: Some("Mic".to_string()),
                ..Default::default()
            },
            AudioInputConfig {
                name: "Mic B".to_string(),
                alias: Some(" mic ".to_string()),
                ..Default::default()
            },
        ];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn duplicate_audio_shortcut_rejected_case_insensitive() {
        let mut c = valid_config();
        c.audio.inputs = vec![
            AudioInputConfig {
                name: "Mic A".to_string(),
                shortcut: Some(" M ".to_string()),
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

    #[test]
    fn blank_audio_alias_rejected() {
        let mut c = valid_config();
        c.audio.inputs = vec![AudioInputConfig {
            name: "Mic".to_string(),
            alias: Some("\t".to_string()),
            ..Default::default()
        }];
        assert!(validate(&c).is_err());
    }

    #[test]
    fn blank_audio_shortcut_rejected() {
        let mut c = valid_config();
        c.audio.inputs = vec![AudioInputConfig {
            name: "Mic".to_string(),
            shortcut: Some(" ".to_string()),
            ..Default::default()
        }];
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
