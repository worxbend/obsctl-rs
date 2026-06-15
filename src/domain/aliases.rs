use super::errors::ObsctlError;
use super::result::Result;

pub struct AliasEntry {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
}

pub fn resolve<'a>(target: &str, entries: &'a [AliasEntry]) -> Result<&'a AliasEntry> {
    let mut candidates: Vec<&AliasEntry> = Vec::new();

    // 1. Exact shortcut
    for e in entries {
        if e.shortcut.as_deref() == Some(target) {
            return Ok(e);
        }
    }

    // 2. Exact alias
    for e in entries {
        if e.alias.as_deref() == Some(target) {
            return Ok(e);
        }
    }

    // 3. Exact OBS name
    for e in entries {
        if e.name == target {
            return Ok(e);
        }
    }

    // 4. Case-insensitive alias
    let target_lower = target.to_lowercase();
    for e in entries {
        if e.alias
            .as_deref()
            .map(|a| a.to_lowercase() == target_lower)
            .unwrap_or(false)
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
        if e.name.to_lowercase() == target_lower {
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
