// Dump-config merge logic: preserves user config while syncing OBS state.
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

use crate::config::model::{AudioInputConfig, Config, SceneConfig};
use crate::domain::aliases::{ensure_unique_aliases_and_shortcuts, normalize_alias_or_shortcut};
use crate::domain::errors::ObsctlError;
use crate::domain::names::{ResourceKind, normalized_name};
use crate::domain::result::Result;
use crate::support::fs;

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

    new_config.scenes = merge_entries(&config.scenes, &obs.scenes)?;
    new_config.audio.inputs = merge_entries(&config.audio.inputs, &obs.inputs)?;

    Ok(new_config)
}

/// The parts of a merge that differ between a scene entry and an audio-input
/// entry.
///
/// Everything else — the collision checks, the stale marking, the appending of
/// newly discovered names, the duplicate check at the end — is the same work
/// on both, so it is written once in [`merge_entries`] and these are the only
/// knobs it turns.
trait MergeableEntry: Clone {
    /// Which kind this is, which decides how it is named in error messages
    /// and which label alias normalization reports against.
    const KIND: ResourceKind;
    /// What OBS calls this kind, for the collision message.
    const OBS_NOUN: &'static str;

    fn name(&self) -> &str;
    fn alias(&self) -> Option<&str>;
    fn shortcut(&self) -> Option<&str>;
    /// A copy of this entry with its stale flag set, keeping everything the
    /// user configured — alias, shortcut, group.
    fn with_stale(&self, stale: bool) -> Self;
    /// A fresh entry for a name OBS has but the config does not, with no alias.
    fn newly_discovered(name: &str) -> Self;
}

impl MergeableEntry for SceneConfig {
    const KIND: ResourceKind = ResourceKind::Scene;
    const OBS_NOUN: &'static str = "OBS scene name";

    fn name(&self) -> &str {
        &self.name
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn shortcut(&self) -> Option<&str> {
        self.shortcut.as_deref()
    }

    fn with_stale(&self, stale: bool) -> Self {
        Self {
            stale,
            ..self.clone()
        }
    }

    fn newly_discovered(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Self::default()
        }
    }
}

impl MergeableEntry for AudioInputConfig {
    const KIND: ResourceKind = ResourceKind::AudioInput;
    const OBS_NOUN: &'static str = "OBS input name";

    fn name(&self) -> &str {
        &self.name
    }

    fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    fn shortcut(&self) -> Option<&str> {
        self.shortcut.as_deref()
    }

    fn with_stale(&self, stale: bool) -> Self {
        Self {
            stale,
            ..self.clone()
        }
    }

    fn newly_discovered(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Self::default()
        }
    }
}

/// Reconcile what the user configured with what OBS currently has.
///
/// Entries OBS still knows keep everything the user set and lose any stale
/// mark; entries OBS no longer has are marked stale rather than deleted, so a
/// scene that is temporarily missing does not cost the user its alias; names
/// OBS has that the config does not are appended.
fn merge_entries<T: MergeableEntry>(existing: &[T], obs_names: &[String]) -> Result<Vec<T>> {
    let obs_set: HashSet<String> = obs_names
        .iter()
        .map(|name| normalized(T::KIND, name))
        .collect::<Result<_>>()?;

    for entry in existing {
        reject_collision_with_obs_name(entry, &obs_set)?;
    }

    let mut merged: Vec<T> = existing
        .iter()
        .map(|entry| Ok(entry.with_stale(!obs_set.contains(&normalized(T::KIND, entry.name())?))))
        .collect::<Result<_>>()?;

    let existing_names: HashSet<String> = existing
        .iter()
        .map(|entry| normalized(T::KIND, entry.name()))
        .collect::<Result<_>>()?;
    for name in obs_names {
        if !existing_names.contains(&normalized(T::KIND, name)?) {
            merged.push(T::newly_discovered(name));
        }
    }

    ensure_unique_aliases_and_shortcuts(
        T::KIND,
        merged.iter().map(|entry| (entry.alias(), entry.shortcut())),
    )?;
    Ok(merged)
}

/// Refuse an alias or shortcut that is already the real name of a *different*
/// OBS resource.
///
/// Allowed to match the entry's own name — writing `alias: main` on the scene
/// called `Main` is redundant but harmless. Pointing it at some other scene is
/// not: the alias would silently shadow that scene.
fn reject_collision_with_obs_name<T: MergeableEntry>(
    entry: &T,
    obs_set: &HashSet<String>,
) -> Result<()> {
    let entry_name = normalized(T::KIND, entry.name())?;

    for (label, value) in [("alias", entry.alias()), ("shortcut", entry.shortcut())] {
        let Some(value) = value else { continue };
        let normalized_value = normalize_alias_or_shortcut(value, T::KIND)?;
        if obs_set.contains(&normalized_value) && normalized_value != entry_name {
            return Err(ObsctlError::ConfigInvalid(format!(
                "{} {label} '{value}' collides with {}",
                T::KIND.label(),
                T::OBS_NOUN
            )));
        }
    }

    Ok(())
}

/// The comparison form of a resource name, reported against the kind it
/// belongs to when it is unusable.
fn normalized(kind: ResourceKind, value: &str) -> Result<String> {
    normalized_name(value)
        .map_err(|error| ObsctlError::ConfigInvalid(format!("{} {error}", kind.name_label())))
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

    /// The audio path has the same alias/shortcut collision rule as the scene
    /// path above. Pinned separately because the two used to be independent
    /// copies of the check, so "scenes are covered" did not imply inputs were.
    #[test]
    fn rejects_audio_alias_collision_with_obs_input_name() {
        let mut config = base_config();
        config.audio.inputs[0].alias = Some("Desktop".to_string());
        let obs = ObsResources {
            scenes: vec![],
            inputs: vec!["Mic".to_string(), "Desktop".to_string()],
        };
        assert!(merge(&config, &obs).is_err());
    }

    #[test]
    fn rejects_audio_shortcut_collision_with_obs_input_name() {
        let mut config = base_config();
        config.audio.inputs[0].shortcut = Some("Desktop".to_string());
        let obs = ObsResources {
            scenes: vec![],
            inputs: vec!["Mic".to_string(), "Desktop".to_string()],
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

    /// `merge` replaces only `scenes` and `audio.inputs`, so scene profiles
    /// ride through a `dump-config` untouched. In particular a profile that
    /// hides a scene OBS no longer has keeps that entry: the scene is marked
    /// stale rather than deleted, and a temporarily missing scene must not
    /// silently cost the user part of a profile.
    #[test]
    fn preserves_scene_profiles_and_the_active_one() {
        use crate::config::model::SceneProfileConfig;

        let mut config = base_config();
        config.scene_profiles = vec![SceneProfileConfig {
            name: "streaming".to_string(),
            hidden: vec!["OldScene".to_string()],
        }];
        config.active_scene_profile = Some("streaming".to_string());

        let obs = ObsResources {
            scenes: vec!["Main".to_string()],
            inputs: vec![],
        };
        let merged = merge(&config, &obs).unwrap();

        assert_eq!(merged.scene_profiles, config.scene_profiles);
        assert_eq!(merged.active_scene_profile.as_deref(), Some("streaming"));
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
