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

/// Which spelling of an entry an exact match is tried against, in order: a
/// shortcut is the most deliberate thing a user can type, an alias next, and
/// the OBS name last.
const EXACT_MATCH_ORDER: [fn(&AliasEntry) -> Option<&str>; 3] = [
    |entry| entry.shortcut.as_deref(),
    |entry| entry.alias.as_deref(),
    |entry| Some(entry.name.as_str()),
];

/// The spellings a case-insensitive match may use, in order.
///
/// Shortcuts are deliberately absent. They are single keystrokes whose whole
/// point is to be distinct, so `m` and `M` are allowed to mean two different
/// things; folding their case would quietly merge them.
const CASE_INSENSITIVE_MATCH_ORDER: [fn(&AliasEntry) -> Option<&str>; 2] = [
    |entry| entry.alias.as_deref(),
    |entry| Some(entry.name.as_str()),
];

/// Resolve a scene target, reporting an unknown name as [`ObsctlError::SceneNotFound`].
pub fn resolve<'a>(target: &str, entries: &'a [AliasEntry]) -> Result<&'a AliasEntry> {
    resolve_with(target, entries, ObsctlError::SceneNotFound)
}

/// Resolve an audio input target, reporting an unknown name as
/// [`ObsctlError::AudioInputNotFound`].
pub fn resolve_audio<'a>(target: &str, entries: &'a [AliasEntry]) -> Result<&'a AliasEntry> {
    resolve_with(target, entries, ObsctlError::AudioInputNotFound)
}

/// Work out which entry the user meant by `target`.
///
/// Matching runs in two rounds. First an exact, case-sensitive pass over
/// shortcuts, then aliases, then OBS names: an exact hit is unambiguous by
/// definition and wins immediately, which is what lets a one-letter shortcut
/// beat some other entry whose name happens to differ only in case.
///
/// Only if nothing matched exactly does a case-insensitive round run, over
/// aliases and then OBS names. There a second match is a genuine ambiguity and
/// is reported rather than guessed at.
///
/// The caller supplies `not_found` because that is the only thing that differs
/// between resolving a scene and resolving an audio input; previously this
/// always reported a missing scene and the audio caller rewrote the error
/// afterwards.
fn resolve_with<'a>(
    target: &str,
    entries: &'a [AliasEntry],
    not_found: fn(String) -> ObsctlError,
) -> Result<&'a AliasEntry> {
    // Validated up front, before any match is attempted, so that a target
    // containing control characters is rejected even when some entry would
    // have matched it exactly.
    let normalized_target = normalized_token(target)?;
    let trimmed_target = target.trim();

    for read in EXACT_MATCH_ORDER {
        if let Some(entry) = entries.iter().find(|e| read(e) == Some(trimmed_target)) {
            return Ok(entry);
        }
    }

    for read in CASE_INSENSITIVE_MATCH_ORDER {
        if let Some(entry) =
            unique_case_insensitive_match(entries, read, &normalized_target, target)?
        {
            return Ok(entry);
        }
    }

    Err(not_found(target.to_string()))
}

/// The one entry whose `read` spelling equals `normalized_target` ignoring
/// case, or `None` if there is no such entry.
///
/// More than one is an error rather than a pick: two entries the user could
/// equally have meant is a config mistake, and silently choosing the first
/// would act on the wrong scene or mute the wrong microphone.
fn unique_case_insensitive_match<'a>(
    entries: &'a [AliasEntry],
    read: fn(&AliasEntry) -> Option<&str>,
    normalized_target: &str,
    raw_target: &str,
) -> Result<Option<&'a AliasEntry>> {
    let mut matched: Option<&AliasEntry> = None;

    for entry in entries {
        let Some(value) = read(entry) else { continue };
        if normalized_token(value)? != normalized_target {
            continue;
        }
        if matched.is_some() {
            return Err(ObsctlError::AliasAmbiguous(raw_target.to_string()));
        }
        matched = Some(entry);
    }

    Ok(matched)
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

    /// Shortcuts are single keystrokes chosen to be distinct, so `m` and `M`
    /// are allowed to mean two different things. Only aliases and OBS names
    /// are matched ignoring case.
    #[test]
    fn shortcut_matching_is_case_sensitive() {
        let entries = vec![entry("Main Scene", None, Some("m"))];
        assert!(matches!(
            resolve("M", &entries),
            Err(ObsctlError::SceneNotFound(_))
        ));
        assert_eq!(resolve("m", &entries).unwrap().name, "Main Scene");
    }

    /// Two entries differing only in case are a config mistake, and the error
    /// has to quote what the user actually typed for them to find it.
    #[test]
    fn ambiguous_error_reports_the_target_as_typed() {
        let entries = vec![
            entry("Scene A", Some("CAM"), None),
            entry("Scene B", Some("Cam"), None),
        ];
        match resolve(" cAm ", &entries).err() {
            Some(ObsctlError::AliasAmbiguous(target)) => assert_eq!(target, " cAm "),
            other => panic!("expected AliasAmbiguous, got {other:?}"),
        }
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
