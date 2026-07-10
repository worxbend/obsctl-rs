// Dump-config merge logic: preserves user config while syncing OBS state.
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use crate::config::model::{AudioInputConfig, Config, SceneConfig};
use crate::domain::aliases::normalize_alias_or_shortcut;
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::support::fs;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// Names discovered from OBS (scenes and audio inputs).
pub struct ObsResources {
    pub scenes: Vec<String>,
    pub inputs: Vec<String>,
}

/// Merge OBS resources into an existing config.
///
/// Rules:
/// - Existing scene/audio entries retain alias, shortcut, group.
/// - Entries still present in OBS have stale cleared.
/// - Entries missing from OBS are marked stale=true.
/// - New OBS resources are appended without aliases.
/// - Duplicate alias/shortcut collisions with OBS names are rejected.
pub fn merge(config: &Config, obs: &ObsResources) -> Result<Config> {
    let mut new_config = config.clone();

    new_config.scenes = merge_scenes(&config.scenes, &obs.scenes)?;
    new_config.audio.inputs = merge_inputs(&config.audio.inputs, &obs.inputs)?;

    Ok(new_config)
}

fn merge_scenes(existing: &[SceneConfig], obs_names: &[String]) -> Result<Vec<SceneConfig>> {
    let obs_set: HashSet<String> = obs_names
        .iter()
        .map(|s| normalized("scene", s))
        .collect::<Result<_>>()?;

    // Validate alias/shortcut collisions with OBS names.
    for sc in existing {
        let scene_name = normalized("scene", &sc.name)?;
        if let Some(alias) = &sc.alias {
            let normalized_alias = normalize_alias_or_shortcut(alias, "scene")?;
            if obs_set.contains(&normalized_alias) && normalized_alias != scene_name {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "scene alias '{}' collides with OBS scene name",
                    alias
                )));
            }
        }
        if let Some(shortcut) = &sc.shortcut {
            let normalized_shortcut = normalize_alias_or_shortcut(shortcut, "scene")?;
            if obs_set.contains(&normalized_shortcut) && normalized_shortcut != scene_name {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "scene shortcut '{}' collides with OBS scene name",
                    shortcut
                )));
            }
        }
    }

    let mut merged: Vec<SceneConfig> = existing
        .iter()
        .map(|sc| {
            let normalized_name = normalized("scene", &sc.name)?;
            let present = obs_set.contains(&normalized_name);
            Ok(SceneConfig {
                stale: !present,
                ..sc.clone()
            })
        })
        .collect::<Result<_>>()?;

    // Append newly discovered scenes.
    let existing_names: HashSet<String> = existing
        .iter()
        .map(|s| normalized("scene", &s.name))
        .collect::<Result<_>>()?;
    for name in obs_names {
        let normalized_name = normalized("scene", name)?;
        if !existing_names.contains(&normalized_name) {
            merged.push(SceneConfig {
                name: name.clone(),
                ..SceneConfig::default()
            });
        }
    }

    validate_no_duplicates(
        "scene",
        merged
            .iter()
            .map(|sc| (sc.alias.as_deref(), sc.shortcut.as_deref())),
    )?;
    Ok(merged)
}

fn merge_inputs(
    existing: &[AudioInputConfig],
    obs_names: &[String],
) -> Result<Vec<AudioInputConfig>> {
    let obs_set: HashSet<String> = obs_names
        .iter()
        .map(|s| normalized("audio", s))
        .collect::<Result<_>>()?;

    // Validate alias/shortcut collisions with OBS names.
    for ai in existing {
        let audio_name = normalized("audio", &ai.name)?;
        if let Some(alias) = &ai.alias {
            let normalized_alias = normalize_alias_or_shortcut(alias, "audio")?;
            if obs_set.contains(&normalized_alias) && normalized_alias != audio_name {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "audio alias '{}' collides with OBS input name",
                    alias
                )));
            }
        }
        if let Some(shortcut) = &ai.shortcut {
            let normalized_shortcut = normalize_alias_or_shortcut(shortcut, "audio")?;
            if obs_set.contains(&normalized_shortcut) && normalized_shortcut != audio_name {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "audio shortcut '{}' collides with OBS input name",
                    shortcut
                )));
            }
        }
    }

    let mut merged: Vec<AudioInputConfig> = existing
        .iter()
        .map(|ai| {
            let normalized_name = normalized("audio", &ai.name)?;
            let present = obs_set.contains(&normalized_name);
            Ok(AudioInputConfig {
                stale: !present,
                ..ai.clone()
            })
        })
        .collect::<Result<_>>()?;

    // Append newly discovered inputs.
    let existing_names: HashSet<String> = existing
        .iter()
        .map(|s| normalized("audio", &s.name))
        .collect::<Result<_>>()?;
    for name in obs_names {
        let normalized_name = normalized("audio", name)?;
        if !existing_names.contains(&normalized_name) {
            merged.push(AudioInputConfig {
                name: name.clone(),
                ..AudioInputConfig::default()
            });
        }
    }

    validate_no_duplicates(
        "audio",
        merged
            .iter()
            .map(|ai| (ai.alias.as_deref(), ai.shortcut.as_deref())),
    )?;
    Ok(merged)
}

fn validate_no_duplicates<'a>(
    label: &str,
    items: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
) -> Result<()> {
    let mut aliases: HashSet<String> = HashSet::new();
    let mut shortcuts: HashSet<String> = HashSet::new();
    for (alias, shortcut) in items {
        if let Some(alias) = alias {
            let normalized_alias = normalize_alias_or_shortcut(alias, label)?;
            if !aliases.insert(normalized_alias) {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "duplicate {label} alias: '{alias}'"
                )));
            }
        }
        if let Some(shortcut) = shortcut {
            let normalized_shortcut = normalize_alias_or_shortcut(shortcut, label)?;
            if !shortcuts.insert(normalized_shortcut) {
                return Err(ObsctlError::ConfigInvalid(format!(
                    "duplicate {label} shortcut: '{shortcut}'"
                )));
            }
        }
    }
    Ok(())
}

fn normalized(kind: &str, value: &str) -> Result<String> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| {
            let target = match kind {
                "audio" => "audio input name",
                _ => "scene name",
            };
            ObsctlError::ConfigInvalid(format!("{target} {error}"))
        })
        .map(|value| value.to_ascii_lowercase())
}

/// Write a timestamped backup of the config file.
///
/// Returns the backup path.
pub fn write_backup(config_path: &Path) -> Result<std::path::PathBuf> {
    if !config_path.exists() {
        return Ok(config_path.with_extension("yml.bak"));
    }

    fs::ensure_path_not_symlink(config_path).map_err(|_| {
        ObsctlError::ConfigInvalid("refusing to back up symlinked config path".to_string())
    })?;
    let parent = config_path.parent().unwrap_or(Path::new("."));
    fs::ensure_private_dir(parent).map_err(|_| {
        ObsctlError::ConfigInvalid("config backup parent directory is unsafe".to_string())
    })?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let stem = config_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("config");
    let backup_name = format!("{stem}.{ts}.bak.yml");
    let backup_path = config_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(backup_name);

    fs::ensure_path_not_symlink(&backup_path).map_err(|_| {
        ObsctlError::ConfigInvalid("refusing to write symlinked backup path".to_string())
    })?;

    let source = fs::read_file_no_follow(config_path).map_err(ObsctlError::Io)?;
    fs::write_atomic_with_temp_file(&backup_path, "obsctl-backup", 0o600, true, |tmp| {
        tmp.write_all(source.as_bytes())?;
        Ok(())
    })
    .map_err(ObsctlError::Io)?;
    Ok(backup_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{AudioInputConfig, Config, SceneConfig};
    use crate::config::writer;
    use crate::support::validation::MAX_TARGET_TOKEN_LENGTH;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn base_config() -> Config {
        use crate::config::model::AudioConfig;
        Config {
            scenes: vec![
                SceneConfig {
                    name: "Main".to_string(),
                    alias: Some("main".to_string()),
                    shortcut: Some("m".to_string()),
                    group: Some("live".to_string()),
                    stale: false,
                    hidden: false,
                },
                SceneConfig {
                    name: "OldScene".to_string(),
                    alias: None,
                    shortcut: None,
                    group: None,
                    stale: false,
                    hidden: false,
                },
            ],
            audio: AudioConfig {
                inputs: vec![
                    AudioInputConfig {
                        name: "Mic".to_string(),
                        alias: Some("mic".to_string()),
                        shortcut: Some("M".to_string()),
                        stale: false,
                    },
                    AudioInputConfig {
                        name: "OldInput".to_string(),
                        alias: None,
                        shortcut: None,
                        stale: false,
                    },
                ],
            },
            ..Config::default()
        }
    }

    #[test]
    fn preserves_alias_and_marks_stale() {
        let config = base_config();
        let obs = ObsResources {
            scenes: vec!["Main".to_string(), "NewScene".to_string()],
            inputs: vec!["Mic".to_string()],
        };
        let merged = merge(&config, &obs).unwrap();

        let main = merged.scenes.iter().find(|s| s.name == "Main").unwrap();
        assert_eq!(main.alias.as_deref(), Some("main"));
        assert_eq!(main.shortcut.as_deref(), Some("m"));
        assert_eq!(main.group.as_deref(), Some("live"));
        assert!(!main.stale);

        let old = merged.scenes.iter().find(|s| s.name == "OldScene").unwrap();
        assert!(old.stale);

        let new = merged.scenes.iter().find(|s| s.name == "NewScene").unwrap();
        assert!(!new.stale);
        assert!(new.alias.is_none());
    }

    #[test]
    fn marks_missing_audio_stale_and_adds_new() {
        let config = base_config();
        let obs = ObsResources {
            scenes: vec!["Main".to_string()],
            inputs: vec!["Mic".to_string(), "Desktop".to_string()],
        };
        let merged = merge(&config, &obs).unwrap();

        let old = merged
            .audio
            .inputs
            .iter()
            .find(|a| a.name == "OldInput")
            .unwrap();
        assert!(old.stale);

        let desktop = merged
            .audio
            .inputs
            .iter()
            .find(|a| a.name == "Desktop")
            .unwrap();
        assert!(!desktop.stale);
        assert!(desktop.alias.is_none());
    }

    #[test]
    fn rejects_alias_collision_with_obs_name() {
        let mut config = base_config();
        config.scenes[0].alias = Some("NewScene".to_string());
        let obs = ObsResources {
            scenes: vec!["Main".to_string(), "NewScene".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_duplicate_scene_alias() {
        let mut config = base_config();
        config.scenes[1].alias = Some("main".to_string()); // duplicate of scenes[0]
        let obs = ObsResources {
            scenes: vec!["Main".to_string(), "OldScene".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_duplicate_scene_alias_case_insensitive() {
        let mut config = base_config();
        config.scenes[1].alias = Some(" mAIN ".to_string()); // duplicate of scenes[0], case/space normalized
        let obs = ObsResources {
            scenes: vec!["Main".to_string(), "OldScene".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_duplicate_audio_alias() {
        let mut config = base_config();
        config.audio.inputs[1].alias = Some("mic".to_string()); // duplicate
        let obs = ObsResources {
            scenes: vec![],
            inputs: vec!["Mic".to_string(), "OldInput".to_string()],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_duplicate_audio_alias_case_insensitive() {
        let mut config = base_config();
        config.audio.inputs[1].alias = Some(" mIC ".to_string()); // duplicate of scenes[0] alias
        let obs = ObsResources {
            scenes: vec![],
            inputs: vec!["Mic".to_string(), "OldInput".to_string()],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_scene_name_with_control_characters() {
        let mut config = base_config();
        config.scenes[0].name = "Main\nScene".to_string();
        let obs = ObsResources {
            scenes: vec!["Main".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_obs_scene_name_with_control_characters() {
        let config = base_config();
        let obs = ObsResources {
            scenes: vec!["Main\t".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_oversized_scene_name_in_config_or_obs() {
        let mut config = base_config();
        config.scenes[0].name = "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1);
        let obs = ObsResources {
            scenes: vec!["Main".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());

        let config = base_config();
        let obs = ObsResources {
            scenes: vec!["a".repeat(MAX_TARGET_TOKEN_LENGTH + 1)],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_blank_alias_in_existing_entry() {
        let mut config = base_config();
        config.scenes[1].alias = Some("   ".to_string());
        let obs = ObsResources {
            scenes: vec!["Main".to_string(), "OldScene".to_string()],
            inputs: vec![],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn backup_creates_timestamped_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yml");
        std::fs::write(&config_path, "version: 1\n").unwrap();

        let backup = write_backup(&config_path).unwrap();
        assert!(backup.exists());
        let name = backup.file_name().unwrap().to_string_lossy();
        assert!(name.contains(".bak.yml"), "backup name: {name}");
    }

    #[test]
    fn backup_replicates_contents() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yml");
        std::fs::write(&config_path, "version: 1\n").unwrap();

        let backup = write_backup(&config_path).unwrap();
        let payload = std::fs::read_to_string(&backup).unwrap();
        assert_eq!(payload, "version: 1\n");
    }

    #[test]
    fn backup_skips_when_no_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("missing.yml");
        let backup = write_backup(&config_path).unwrap();
        assert!(!backup.exists());
    }

    #[cfg(unix)]
    #[test]
    fn backup_rejects_symlinked_config_path() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real-config.yml");
        let link = dir.path().join("link-config.yml");
        std::fs::write(&real, "version: 1\n").unwrap();
        symlink(&real, &link).unwrap();

        let err = write_backup(&link).unwrap_err();
        assert!(
            err.to_string()
                .contains("refusing to back up symlinked config path")
        );
    }

    #[cfg(unix)]
    #[test]
    fn backup_sets_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.yml");
        std::fs::write(&config_path, "version: 1\n").unwrap();

        let backup = write_backup(&config_path).unwrap();
        let metadata = std::fs::metadata(&backup).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn write_atomic_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        let config = Config::default();
        writer::write_atomic(&config, &path).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("yml.tmp").exists());
    }

    #[test]
    fn all_stale_when_obs_is_empty() {
        let config = base_config();
        let obs = ObsResources {
            scenes: vec![],
            inputs: vec![],
        };
        let merged = merge(&config, &obs).unwrap();
        assert!(merged.scenes.iter().all(|s| s.stale));
        assert!(merged.audio.inputs.iter().all(|a| a.stale));
    }

    #[test]
    fn preserves_server_and_reconnect_settings() {
        let mut config = base_config();
        config.server.allow_remote_shutdown = true;
        config.reconnect.initial_delay_ms = 1000;
        config.ui.theme = "dark".to_string();
        let obs = ObsResources {
            scenes: vec![],
            inputs: vec![],
        };
        let merged = merge(&config, &obs).unwrap();
        assert!(merged.server.allow_remote_shutdown);
        assert_eq!(merged.reconnect.initial_delay_ms, 1000);
        assert_eq!(merged.ui.theme, "dark");
    }
}
