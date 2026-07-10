use super::errors::ObsctlError;
use super::result::Result;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

pub struct AliasEntry {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
}

fn normalized_token(value: &str) -> Result<String> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::ConfigInvalid(format!("alias or shortcut {error}")))
        .map(|value| value.to_ascii_lowercase())
}

pub fn normalize_alias_or_shortcut(value: &str, kind: &str) -> Result<String> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::ConfigInvalid(format!("{kind} aliases/shortcuts {error}")))
        .map(|value| value.to_ascii_lowercase())
}

pub fn resolve<'a>(target: &str, entries: &'a [AliasEntry]) -> Result<&'a AliasEntry> {
    let target_normalized = normalized_token(target)?;
    let target_trimmed = target.trim();
    let mut candidates: Vec<&AliasEntry> = Vec::new();

    // 1. Exact shortcut
    for e in entries {
        if e.shortcut.as_deref() == Some(target_trimmed) {
            return Ok(e);
        }
    }

    // 2. Exact alias
    for e in entries {
        if e.alias.as_deref() == Some(target_trimmed) {
            return Ok(e);
        }
    }

    // 3. Exact OBS name
    for e in entries {
        if e.name == target_trimmed {
            return Ok(e);
        }
    }

    // 4. Case-insensitive alias
    for e in entries {
        if let Some(alias) = e.alias.as_deref()
            && normalized_token(alias)? == target_normalized
        {
            candidates.push(e);
        }
    }

    if candidates.len() == 1 {
        return Ok(candidates[0]);
    } else if candidates.len() > 1 {
        return Err(ObsctlError::AliasAmbiguous(target.to_string()));
    }

    // 5. Case-insensitive OBS name
    for e in entries {
        if normalized_token(&e.name)? == target_normalized {
            candidates.push(e);
        }
    }

    match candidates.len() {
        1 => Ok(candidates[0]),
        0 => Err(ObsctlError::SceneNotFound(target.to_string())),
        _ => Err(ObsctlError::AliasAmbiguous(target.to_string())),
    }
}

/// Resolve an audio input target, returning `AudioInputNotFound` instead of `SceneNotFound`.
pub fn resolve_audio<'a>(target: &str, entries: &'a [AliasEntry]) -> Result<&'a AliasEntry> {
    resolve(target, entries).map_err(|e| match e {
        ObsctlError::SceneNotFound(t) => ObsctlError::AudioInputNotFound(t),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, alias: Option<&str>, shortcut: Option<&str>) -> AliasEntry {
        AliasEntry {
            name: name.to_string(),
            alias: alias.map(String::from),
            shortcut: shortcut.map(String::from),
        }
    }

    #[test]
    fn exact_shortcut_wins() {
        let entries = vec![entry("Main Scene", Some("main"), Some("m"))];
        assert_eq!(resolve("m", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn exact_alias_wins() {
        let entries = vec![entry("Main Scene", Some("main"), None)];
        assert_eq!(resolve("main", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn exact_obs_name() {
        let entries = vec![entry("Main Scene", None, None)];
        assert_eq!(resolve("Main Scene", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn case_insensitive_alias_match() {
        let entries = vec![entry("Main Scene", Some("MainCam"), None)];
        assert_eq!(resolve("maincam", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn case_insensitive_obs_name_match() {
        let entries = vec![entry("Main Scene", None, None)];
        assert_eq!(resolve("main scene", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn not_found() {
        let entries = vec![entry("Main Scene", None, None)];
        assert!(matches!(
            resolve("Other", &entries),
            Err(ObsctlError::SceneNotFound(_))
        ));
    }

    #[test]
    fn ambiguous_alias_fails() {
        // Neither alias matches exactly (case-sensitive), but both match case-insensitively.
        let entries = vec![
            entry("Scene A", Some("CAM"), None),
            entry("Scene B", Some("Cam"), None),
        ];
        assert!(matches!(
            resolve("cam", &entries),
            Err(ObsctlError::AliasAmbiguous(_))
        ));
    }

    #[test]
    fn resolve_allows_target_whitespace_trimming_for_exact_match_fallback() {
        let entries = vec![entry("Main Scene", Some(" main "), None)];
        assert_eq!(resolve("main", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn resolve_case_insensitive_match_uses_trimmed_alias() {
        let entries = vec![entry("Main Scene", Some("MainCam"), None)];
        assert_eq!(resolve(" maincam ", &entries).unwrap().name, "Main Scene");
    }

    #[test]
    fn resolve_rejects_target_with_control_characters() {
        let entries = vec![entry("Main Scene", Some("main"), None)];
        assert!(matches!(
            resolve("main\t", &entries),
            Err(ObsctlError::ConfigInvalid(_))
        ));
    }

    #[test]
    fn reject_alias_with_control_character() {
        assert!(matches!(
            normalize_alias_or_shortcut("bad\talias", "scene"),
            Err(ObsctlError::ConfigInvalid(_))
        ));
    }

    #[test]
    fn reject_alias_with_excessive_length() {
        let value = "a".repeat(crate::support::validation::MAX_TARGET_TOKEN_LENGTH + 1);
        assert!(matches!(
            normalize_alias_or_shortcut(&value, "scene"),
            Err(ObsctlError::ConfigInvalid(_))
        ));
    }

    #[test]
    fn audio_not_found_returns_audio_error() {
        let entries: Vec<AliasEntry> = vec![];
        assert!(matches!(
            resolve_audio("Mic", &entries),
            Err(ObsctlError::AudioInputNotFound(_))
        ));
    }

    #[test]
    fn shortcut_takes_priority_over_alias() {
        let entries = vec![
            entry("By Shortcut", None, Some("x")),
            entry("By Alias", Some("x"), None),
        ];
        assert_eq!(resolve("x", &entries).unwrap().name, "By Shortcut");
    }
}
