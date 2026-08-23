use std::collections::HashSet;

use crate::domain::aliases::ensure_unique_aliases_and_shortcuts;
use crate::domain::errors::ObsctlError;
use crate::domain::names::{ResourceKind, checked_name, normalized_name};
use crate::domain::result::Result;
use crate::ipc::socket_path::resolve_server_socket_path;
use crate::support::validation::{
    password_config_error_message, resolve_connection_password, validate_no_control_or_whitespace,
};

use super::model::{Config, ConnectionConfig};

#[derive(Debug)]
pub struct ValidationWarning(pub String);

/// Check a whole config, in the order the sections appear below.
///
/// The distinction between the two ways of reporting a problem is the point of
/// this module. An `Err` means the config cannot be used and the command
/// stops. A `ValidationWarning` means it can be used but something in it is
/// unwise or will be ignored — a plaintext password, an unusable palette
/// prefix — and the user is told while the daemon carries on. Turning an
/// existing warning into an error breaks configs that work today, so the
/// choice per check is deliberate rather than incidental.
///
/// Order matters and is preserved: a config wrong in more than one way reports
/// the first problem in this sequence, and tests pin that.
pub fn validate(config: &Config) -> Result<Vec<ValidationWarning>> {
    let mut warnings = Vec::new();

    validate_version(config.version)?;
    validate_connection_config(&config.connection)?;
    validate_resource_names(config)?;
    warnings.extend(validate_ui_config(&config.ui)?);
    validate_reconnect_config(&config.reconnect)?;
    validate_socket_path(config)?;
    warnings.extend(validate_password_config(&config.connection)?);
    validate_no_duplicate_aliases(config)?;
    warnings.extend(validate_scene_profiles(config)?);

    Ok(warnings)
}

fn config_invalid(message: impl Into<String>) -> ObsctlError {
    ObsctlError::ConfigInvalid(message.into())
}

fn validate_version(version: u32) -> Result<()> {
    if version != 1 {
        return Err(config_invalid(format!(
            "unsupported config version: {version}"
        )));
    }
    Ok(())
}

fn validate_ui_config(ui: &super::model::UiConfig) -> Result<Vec<ValidationWarning>> {
    if ui.refresh_interval_ms == 0 {
        return Err(config_invalid("ui.refresh_interval_ms must be positive"));
    }

    let mut warnings = Vec::new();

    // The prefix is stripped before parsing (see `domain::parser`), so only
    // the characters that round-trip through it work. Warn rather than fail:
    // this field went unread for several releases, so an unusable value in an
    // existing config must not start blocking the daemon.
    if !crate::domain::parser::PALETTE_PREFIXES
        .iter()
        .any(|c| ui.command_palette_prefix == c.to_string())
    {
        let supported: Vec<String> = crate::domain::parser::PALETTE_PREFIXES
            .iter()
            .map(char::to_string)
            .collect();
        warnings.push(ValidationWarning(format!(
            "ui.command_palette_prefix '{}' is not supported (supported: {}); falling back to '{}'",
            ui.command_palette_prefix,
            supported.join(", "),
            crate::domain::parser::DEFAULT_PALETTE_PREFIX,
        )));
    }

    if let Some(locale) = &ui.locale
        && !crate::localization::SUPPORTED_LOCALES.contains(&locale.to_ascii_lowercase().as_str())
    {
        warnings.push(ValidationWarning(format!(
            "ui.locale '{locale}' is not supported (supported: {}); falling back to en",
            crate::localization::SUPPORTED_LOCALES.join(", ")
        )));
    }

    Ok(warnings)
}

fn validate_reconnect_config(reconnect: &super::model::ReconnectConfig) -> Result<()> {
    if reconnect.max_delay_ms < reconnect.initial_delay_ms {
        return Err(config_invalid(
            "reconnect.max_delay_ms must be >= initial_delay_ms",
        ));
    }

    if reconnect.multiplier < 1.0 {
        return Err(config_invalid("reconnect.multiplier must be >= 1.0"));
    }

    Ok(())
}

fn validate_socket_path(config: &Config) -> Result<()> {
    resolve_server_socket_path(config.server.socket_path.as_deref())
        .map(|_| ())
        .map_err(|error| config_invalid(format!("server.socket_path {error}")))
}

/// Where the OBS password comes from, and whether that choice is a good one.
///
/// A password that cannot be resolved at all is an error. A password that
/// resolves but was written in plaintext, or an env var that is simply not
/// set, are warnings: both leave a config that still connects.
fn validate_password_config(connection: &ConnectionConfig) -> Result<Vec<ValidationWarning>> {
    let resolved_password =
        resolve_connection_password(connection.password.as_deref(), &connection.password_env)
            .map_err(|error| config_invalid(password_config_error_message(&error)))?;

    let mut warnings = Vec::new();

    if connection.password.is_some() {
        warnings.push(ValidationWarning(
            "connection.password contains a plaintext password; use password_env instead"
                .to_string(),
        ));
    }

    if !connection.password_env.is_empty()
        && connection.password.is_none()
        && resolved_password.is_none()
    {
        warnings.push(ValidationWarning(format!(
            "password_env {} is not set; will connect without a password",
            connection.password_env
        )));
    }

    Ok(warnings)
}

pub(crate) fn validate_connection_config(config: &ConnectionConfig) -> Result<()> {
    validate_connection_host(&config.host)?;

    if config.port == 0 {
        return Err(config_invalid(
            "connection.port must be in range 1-65535".to_string(),
        ));
    }

    if config.connect_timeout_ms == 0 {
        return Err(config_invalid(
            "connection.connect_timeout_ms must be positive".to_string(),
        ));
    }

    if config.request_timeout_ms == 0 {
        return Err(config_invalid(
            "connection.request_timeout_ms must be positive".to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn validate_connection_host(host: &str) -> Result<()> {
    validate_no_control_or_whitespace(host).map_err(|_| {
        config_invalid(
            "connection.host must not contain control or whitespace characters".to_string(),
        )
    })?;

    if host.contains('/') || host.contains('\\') || host.contains(':') || host.contains('@') {
        return Err(config_invalid(
            "connection.host must not contain path, separator, userinfo, or colon characters"
                .to_string(),
        ));
    }

    if host.is_empty() {
        return Err(config_invalid(
            "connection.host must not be blank".to_string(),
        ));
    }

    if host.len() > 253 {
        return Err(config_invalid(
            "connection.host exceeds DNS-typical length limit".to_string(),
        ));
    }

    Ok(())
}

/// No two scenes, and no two audio inputs, may answer to the same alias or
/// shortcut.
///
/// The check itself lives in [`crate::domain::aliases`] next to the resolver
/// that depends on it; this function only decides which entries to feed it.
fn validate_no_duplicate_aliases(config: &Config) -> Result<()> {
    ensure_unique_aliases_and_shortcuts(
        ResourceKind::Scene,
        config
            .scenes
            .iter()
            .map(|s| (s.alias.as_deref(), s.shortcut.as_deref())),
    )?;
    ensure_unique_aliases_and_shortcuts(
        ResourceKind::AudioInput,
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
        checked_name(&scene.name).map_err(|error| config_invalid(format!("scene name {error}")))?;
    }

    for audio in &config.audio.inputs {
        checked_name(&audio.name).map_err(|error| config_invalid(format!("audio name {error}")))?;
    }

    for profile in &config.scene_profiles {
        checked_name(&profile.name)
            .map_err(|error| config_invalid(format!("scene profile name {error}")))?;
        for scene in &profile.hidden {
            checked_name(scene).map_err(|error| {
                config_invalid(format!(
                    "scene profile '{}' hidden scene {error}",
                    profile.name
                ))
            })?;
        }
    }

    Ok(())
}

/// Scene profiles: names must be distinct, and the selected one should exist.
///
/// Only the duplicate is an error. Two profiles answering to the same name
/// make `active_scene_profile` ambiguous and there is no sensible way to pick
/// one. Everything else here is a warning, because the daemon has a defined
/// answer in each case — fall back to the per-scene `hidden` flags — and a
/// config that has been working must not stop the daemon over a scene that is
/// merely absent right now.
///
/// Runs last in [`validate`] so that a config wrong in more than one way still
/// reports whichever earlier problem it reported before scene profiles existed.
fn validate_scene_profiles(config: &Config) -> Result<Vec<ValidationWarning>> {
    let mut seen: HashSet<String> = HashSet::new();
    for profile in &config.scene_profiles {
        // `validate_resource_names` already rejected any name this cannot
        // normalize, so a failure here cannot happen; treating it as "no
        // duplicate" keeps this function from having to invent a second
        // message for a case the earlier check owns.
        let Ok(normalized) = normalized_name(&profile.name) else {
            continue;
        };
        if !seen.insert(normalized) {
            return Err(config_invalid(format!(
                "duplicate scene profile name: '{}'",
                profile.name
            )));
        }
    }

    let mut warnings = Vec::new();

    if let Some(active) = &config.active_scene_profile
        && config.active_scene_profile().is_none()
    {
        warnings.push(ValidationWarning(format!(
            "active scene profile '{active}' is not defined; no scenes are hidden by a profile"
        )));
    }

    // An empty `scenes:` list is not a claim that there are no scenes — it is
    // the shipped default, and what `obsctl init` writes. Only `dump-config`
    // ever fills it in. Cross-checking a profile against it in that state
    // would report every scene a profile hides as unknown, which is one
    // warning per hidden scene in the server log and the TUI log pane on every
    // save, reload and validate, about scenes that are perfectly real. With no
    // list to check against there is no ground truth, so there is nothing to
    // say.
    if config.scenes.is_empty() {
        return Ok(warnings);
    }

    let configured_scenes: HashSet<String> = config
        .scenes
        .iter()
        .filter_map(|scene| normalized_name(&scene.name).ok())
        .collect();
    for profile in &config.scene_profiles {
        for scene in &profile.hidden {
            let known = normalized_name(scene)
                .is_ok_and(|normalized| configured_scenes.contains(&normalized));
            if !known {
                warnings.push(ValidationWarning(format!(
                    "scene profile '{}' hides '{scene}', which is not in the scenes list",
                    profile.name
                )));
            }
        }
    }

    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{AudioInputConfig, SceneConfig, SceneProfileConfig};
    use crate::support::validation::test_env::with_env_var;
    use crate::support::validation::{MAX_PASSWORD_LENGTH, MAX_TARGET_TOKEN_LENGTH};
    use tempfile::TempDir;

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
    fn both_palette_prefixes_validate_without_warning() {
        for prefix in ["/", ":"] {
            let mut c = valid_config();
            c.ui.command_palette_prefix = prefix.to_string();
            let warnings = validate(&c).expect("prefix should be accepted");
            assert!(
                !warnings
                    .iter()
                    .any(|w| w.0.contains("command_palette_prefix")),
                "'{prefix}' should not warn, got {warnings:?}"
            );
        }
    }

    #[test]
    fn an_unusable_palette_prefix_warns_instead_of_failing() {
        let mut c = valid_config();
        c.ui.command_palette_prefix = ">>".to_string();
        let warnings = validate(&c).expect("an unusable prefix must not block startup");
        assert!(
            warnings
                .iter()
                .any(|w| w.0.contains("command_palette_prefix")),
            "expected a fallback warning, got {warnings:?}"
        );
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
            name: "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1),
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
            name: "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1),
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

    fn scene_profile(name: &str, hidden: &[&str]) -> SceneProfileConfig {
        SceneProfileConfig {
            name: name.to_string(),
            hidden: hidden.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A config that lists one scene and one profile hiding it, so that scene
    /// profile tests start from a state with no warnings of their own.
    fn config_with_one_profile() -> Config {
        let mut c = valid_config();
        c.scenes = vec![SceneConfig {
            name: "Utility BG".to_string(),
            ..Default::default()
        }];
        c.scene_profiles = vec![scene_profile("streaming", &["Utility BG"])];
        c
    }

    #[test]
    fn a_valid_scene_profile_produces_no_warnings() {
        let mut c = config_with_one_profile();
        c.active_scene_profile = Some("streaming".to_string());
        let warnings = validate(&c).expect("a well-formed scene profile should validate");
        assert!(
            !warnings.iter().any(|w| w.0.contains("scene profile")),
            "unexpected scene profile warning: {warnings:?}"
        );
    }

    #[test]
    fn duplicate_scene_profile_names_rejected() {
        let mut c = config_with_one_profile();
        c.scene_profiles
            .push(scene_profile("streaming", &["Utility BG"]));
        let err = validate(&c).unwrap_err();
        assert!(err.to_string().contains("duplicate scene profile name"));
    }

    #[test]
    fn duplicate_scene_profile_names_rejected_case_insensitive() {
        let mut c = config_with_one_profile();
        c.scene_profiles
            .push(scene_profile(" Streaming ", &["Utility BG"]));
        assert!(validate(&c).is_err());
    }

    #[test]
    fn scene_profile_name_rejects_blank_and_control_characters() {
        for name in ["   ", "record\nonly"] {
            let mut c = valid_config();
            c.scene_profiles = vec![scene_profile(name, &[])];
            let err = validate(&c).unwrap_err();
            assert!(
                err.to_string().contains("scene profile name"),
                "name {name:?} gave {err}"
            );
        }
    }

    #[test]
    fn scene_profile_hidden_entry_rejects_control_characters() {
        let mut c = valid_config();
        c.scene_profiles = vec![scene_profile("streaming", &["Utility\nBG"])];
        let err = validate(&c).unwrap_err();
        assert!(err.to_string().contains("hidden scene"), "got {err}");
    }

    /// A profile that has been deleted, or a name typed with a typo, leaves
    /// the daemon with a defined answer — the per-scene flags — so it must not
    /// stop startup.
    #[test]
    fn an_unknown_active_scene_profile_warns_instead_of_failing() {
        let mut c = config_with_one_profile();
        c.active_scene_profile = Some("no such profile".to_string());
        let warnings = validate(&c).expect("an unknown scene profile must not block startup");
        assert!(
            warnings.iter().any(|w| w
                .0
                .contains("active scene profile 'no such profile' is not defined")),
            "expected a fallback warning, got {warnings:?}"
        );
    }

    #[test]
    fn a_profile_hiding_an_unconfigured_scene_warns() {
        let mut c = config_with_one_profile();
        c.scene_profiles[0].hidden.push("Gone Scene".to_string());
        let warnings = validate(&c).expect("a missing scene must not block startup");
        assert!(
            warnings.iter().any(|w| w
                .0
                .contains("hides 'Gone Scene', which is not in the scenes list")),
            "expected a stale-scene warning, got {warnings:?}"
        );
    }

    /// An empty `scenes:` is the shipped default and what `obsctl init`
    /// writes; only `dump-config` ever fills it in. With no list to check
    /// against there is no ground truth, so a profile hiding real scenes must
    /// not produce one "not in the scenes list" warning per scene on every
    /// save, reload and validate.
    #[test]
    fn a_profile_hiding_scenes_warns_about_none_of_them_when_scenes_is_empty() {
        let mut c = Config::default();
        c.connection.password_env = String::new();
        c.scene_profiles = vec![SceneProfileConfig {
            name: "streaming".to_string(),
            hidden: vec![
                "Utility BG".to_string(),
                "BRB".to_string(),
                "Cam Only".to_string(),
            ],
        }];

        let warnings = validate(&c).expect("this config is valid");
        assert!(
            warnings.is_empty(),
            "an empty scenes list is not evidence a scene is missing, got {warnings:?}"
        );
    }

    /// Scene profile checks run last, so a config that is also wrong in an
    /// older way still reports the older problem first.
    #[test]
    fn scene_profile_checks_do_not_pre_empt_earlier_errors() {
        let mut c = config_with_one_profile();
        c.scene_profiles
            .push(scene_profile("streaming", &["Utility BG"]));
        c.connection.host = String::new();
        let err = validate(&c).unwrap_err();
        assert!(err.to_string().contains("connection.host"), "got {err}");
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
