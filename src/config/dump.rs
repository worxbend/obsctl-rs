// Dump-config merge logic: preserves user config while syncing OBS state.
use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;

use crate::config::model::{AudioInputConfig, Config, SceneConfig};
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

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
    let obs_set: HashSet<&str> = obs_names.iter().map(|s| s.as_str()).collect();

    // Validate alias/shortcut collisions with OBS names.
    for sc in existing {
        if let Some(alias) = &sc.alias
            && obs_set.contains(alias.as_str())
            && alias != &sc.name
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "scene alias '{}' collides with OBS scene name",
                alias
            )));
        }
        if let Some(shortcut) = &sc.shortcut
            && obs_set.contains(shortcut.as_str())
            && shortcut != &sc.name
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "scene shortcut '{}' collides with OBS scene name",
                shortcut
            )));
        }
    }

    let mut merged: Vec<SceneConfig> = existing
        .iter()
        .map(|sc| {
            let present = obs_set.contains(sc.name.as_str());
            SceneConfig {
                stale: !present,
                ..sc.clone()
            }
        })
        .collect();

    // Append newly discovered scenes.
    let existing_names: HashSet<&str> = existing.iter().map(|s| s.name.as_str()).collect();
    for name in obs_names {
        if !existing_names.contains(name.as_str()) {
            merged.push(SceneConfig {
                name: name.clone(),
                ..SceneConfig::default()
            });
        }
    }

    validate_scene_duplicates(&merged)?;
    Ok(merged)
}

fn merge_inputs(
    existing: &[AudioInputConfig],
    obs_names: &[String],
) -> Result<Vec<AudioInputConfig>> {
    let obs_set: HashSet<&str> = obs_names.iter().map(|s| s.as_str()).collect();

    // Validate alias/shortcut collisions with OBS names.
    for ai in existing {
        if let Some(alias) = &ai.alias
            && obs_set.contains(alias.as_str())
            && alias != &ai.name
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "audio alias '{}' collides with OBS input name",
                alias
            )));
        }
        if let Some(shortcut) = &ai.shortcut
            && obs_set.contains(shortcut.as_str())
            && shortcut != &ai.name
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "audio shortcut '{}' collides with OBS input name",
                shortcut
            )));
        }
    }

    let mut merged: Vec<AudioInputConfig> = existing
        .iter()
        .map(|ai| {
            let present = obs_set.contains(ai.name.as_str());
            AudioInputConfig {
                stale: !present,
                ..ai.clone()
            }
        })
        .collect();

    // Append newly discovered inputs.
    let existing_names: HashSet<&str> = existing.iter().map(|s| s.name.as_str()).collect();
    for name in obs_names {
        if !existing_names.contains(name.as_str()) {
            merged.push(AudioInputConfig {
                name: name.clone(),
                ..AudioInputConfig::default()
            });
        }
    }

    validate_audio_duplicates(&merged)?;
    Ok(merged)
}

fn validate_scene_duplicates(scenes: &[SceneConfig]) -> Result<()> {
    let mut aliases = HashSet::new();
    let mut shortcuts = HashSet::new();
    for sc in scenes {
        if let Some(alias) = &sc.alias
            && !aliases.insert(alias.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate scene alias: '{alias}'"
            )));
        }
        if let Some(shortcut) = &sc.shortcut
            && !shortcuts.insert(shortcut.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate scene shortcut: '{shortcut}'"
            )));
        }
    }
    Ok(())
}

fn validate_audio_duplicates(inputs: &[AudioInputConfig]) -> Result<()> {
    let mut aliases = HashSet::new();
    let mut shortcuts = HashSet::new();
    for ai in inputs {
        if let Some(alias) = &ai.alias
            && !aliases.insert(alias.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate audio alias: '{alias}'"
            )));
        }
        if let Some(shortcut) = &ai.shortcut
            && !shortcuts.insert(shortcut.clone())
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate audio shortcut: '{shortcut}'"
            )));
        }
    }
    Ok(())
}

/// Write a timestamped backup of the config file.
///
/// Returns the backup path.
pub fn write_backup(config_path: &Path) -> Result<std::path::PathBuf> {
    if !config_path.exists() {
        return Ok(config_path.with_extension("yml.bak"));
    }

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

    std::fs::copy(config_path, &backup_path)?;
    Ok(backup_path)
}

/// Write config atomically (tmp -> rename).
pub fn write_atomic(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content =
        serde_yaml::to_string(config).map_err(|e| ObsctlError::ConfigInvalid(e.to_string()))?;
    let tmp = path.with_extension("yml.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.flush()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{AudioInputConfig, Config, SceneConfig};

    fn base_config() -> Config {
        use crate::config::model::{AudioConfig, KeymapConfig, UiConfig};
        Config {
            scenes: vec![
                SceneConfig {
                    name: "Main".to_string(),
                    alias: Some("main".to_string()),
                    shortcut: Some("m".to_string()),
                    group: Some("live".to_string()),
                    stale: false,
                },
                SceneConfig {
                    name: "OldScene".to_string(),
                    alias: None,
                    shortcut: None,
                    group: None,
                    stale: false,
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
    fn backup_skips_when_no_config_exists() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("missing.yml");
        let backup = write_backup(&config_path).unwrap();
        assert!(!backup.exists());
    }

    #[test]
    fn write_atomic_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        let config = Config::default();
        write_atomic(&config, &path).unwrap();
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
