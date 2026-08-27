use std::collections::HashSet;

use super::errors::ObsctlError;
use super::names::{ResourceKind, normalized_name};
use super::result::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasEntry {
    pub name: String,
    pub alias: Option<String>,
    pub shortcut: Option<String>,
}

/// The comparison form of a value being matched against an entry, when the
/// caller has no particular kind of resource in hand.
fn normalized_token(value: &str) -> Result<String> {
    normalized_name(value)
        .map_err(|error| ObsctlError::ConfigInvalid(format!("alias or shortcut {error}")))
}

/// The comparison form of an alias or shortcut read out of the config file.
///
/// Same rule as [`normalized_token`]; only the message differs, because here
/// the caller knows whether the offending value sits under `scenes:` or under
/// `audio.inputs:` and saying so is what lets the user find it.
pub fn normalize_alias_or_shortcut(value: &str, kind: ResourceKind) -> Result<String> {
    normalized_name(value).map_err(|error| {
        ObsctlError::ConfigInvalid(format!("{} aliases/shortcuts {error}", kind.label()))
    })
}

/// Refuse a set of entries in which two of them answer to the same alias, or
/// to the same shortcut.
///
/// Values are compared in their normalized form, so ` mAIN ` and `main` are
/// the same alias. Shortcuts are compared that way too: `M` and `m` may mean
/// different things when *resolving* a target, but two entries claiming them
/// as shortcuts is still close enough to be a config mistake.
///
/// This lives next to [`resolve`] because it is the same rule seen from the
/// other side. `resolve` refuses to guess between two entries a target could
/// equally have meant and reports [`ObsctlError::AliasAmbiguous`]; checking
/// uniqueness when a config is loaded or rewritten is what keeps a user from
/// ever reaching that error. The check used to exist as two independent
/// copies — one in config validation, one in the dump-config merge — so
/// correcting either one left the other unchanged.
pub fn ensure_unique_aliases_and_shortcuts<'a>(
    kind: ResourceKind,
    entries: impl Iterator<Item = (Option<&'a str>, Option<&'a str>)>,
) -> Result<()> {
    let label = kind.label();
    let mut aliases: HashSet<String> = HashSet::new();
    let mut shortcuts: HashSet<String> = HashSet::new();

    for (alias, shortcut) in entries {
        if let Some(alias) = alias
            && !aliases.insert(normalize_alias_or_shortcut(alias, kind)?)
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate {label} alias: '{alias}'"
            )));
        }
        if let Some(shortcut) = shortcut
            && !shortcuts.insert(normalize_alias_or_shortcut(shortcut, kind)?)
        {
            return Err(ObsctlError::ConfigInvalid(format!(
                "duplicate {label} shortcut: '{shortcut}'"
            )));
        }
    }

    Ok(())
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
    resolve_with(target, entries, ResourceKind::Scene)
}

/// Resolve an audio input target, reporting an unknown name as
/// [`ObsctlError::AudioInputNotFound`].
pub fn resolve_audio<'a>(target: &str, entries: &'a [AliasEntry]) -> Result<&'a AliasEntry> {
    resolve_with(target, entries, ResourceKind::AudioInput)
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
/// The caller supplies `kind` because which "not found" error a miss reports
/// is the only thing that differs between resolving a scene and resolving an
/// audio input; previously this always reported a missing scene and the audio
/// caller rewrote the error afterwards. See
/// [`ResourceKind::not_found`](super::names::ResourceKind::not_found).
fn resolve_with<'a>(
    target: &str,
    entries: &'a [AliasEntry],
    kind: ResourceKind,
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

    Err(kind.not_found(target.to_string()))
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
        assert_eq!(resolve("m", &entries).unwrap(), &entries[0]);
    }

    #[test]
    fn exact_alias_wins() {
        let entries = vec![entry("Main Scene", Some("main"), None)];
        assert_eq!(resolve("main", &entries).unwrap(), &entries[0]);
    }

    #[test]
    fn exact_obs_name() {
        let entries = vec![entry("Main Scene", None, None)];
        assert_eq!(resolve("Main Scene", &entries).unwrap(), &entries[0]);
    }

    #[test]
    fn case_insensitive_alias_match() {
        let entries = vec![entry("Main Scene", Some("MainCam"), None)];
        assert_eq!(resolve("maincam", &entries).unwrap(), &entries[0]);
    }

    #[test]
    fn case_insensitive_obs_name_match() {
        let entries = vec![entry("Main Scene", None, None)];
        assert_eq!(resolve("main scene", &entries).unwrap(), &entries[0]);
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
        assert_eq!(resolve("main", &entries).unwrap(), &entries[0]);
    }

    #[test]
    fn resolve_case_insensitive_match_uses_trimmed_alias() {
        let entries = vec![entry("Main Scene", Some("MainCam"), None)];
        assert_eq!(resolve(" maincam ", &entries).unwrap(), &entries[0]);
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
            normalize_alias_or_shortcut("bad\talias", ResourceKind::Scene),
            Err(ObsctlError::ConfigInvalid(_))
        ));
    }

    #[test]
    fn reject_alias_with_excessive_length() {
        let value = "a".repeat(crate::support::validation::MAX_TARGET_TOKEN_LENGTH + 1);
        assert!(matches!(
            normalize_alias_or_shortcut(&value, ResourceKind::Scene),
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
        assert_eq!(resolve("m", &entries).unwrap(), &entries[0]);
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

    /// The uniqueness rule is shared by config validation and the dump-config
    /// merge, so it is pinned here at its own level rather than only through
    /// those two callers.
    #[test]
    fn uniqueness_folds_case_and_whitespace() {
        let entries = [(Some("Main"), None), (Some(" mAIN "), None)];
        let error = ensure_unique_aliases_and_shortcuts(
            ResourceKind::Scene,
            entries.iter().map(|(a, s)| (*a, *s)),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("duplicate scene alias"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn uniqueness_reports_the_kind_it_was_given() {
        let entries = [(None, Some("m")), (None, Some("m"))];
        let error = ensure_unique_aliases_and_shortcuts(
            ResourceKind::AudioInput,
            entries.iter().map(|(a, s)| (*a, *s)),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("duplicate audio shortcut"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn uniqueness_accepts_distinct_aliases_and_shortcuts() {
        let entries = [(Some("main"), Some("m")), (Some("cam"), Some("c"))];
        assert!(
            ensure_unique_aliases_and_shortcuts(
                ResourceKind::Scene,
                entries.iter().map(|(a, s)| (*a, *s))
            )
            .is_ok()
        );
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
