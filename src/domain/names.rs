//! What counts as a usable name for an OBS resource, and how such a name is
//! spoken about in an error message.
//!
//! Scene names, audio-input names, aliases, shortcuts and the target of a
//! palette command are all the same shape of value — a short piece of text a
//! user typed or OBS reported — and they all have to pass the same check:
//! trimmed of surrounding whitespace, no control characters, not blank, and no
//! longer than [`MAX_TARGET_TOKEN_LENGTH`]. That check used to be spelled out
//! at six separate call sites, each of which could have drifted from the
//! others. It is performed here once.
//!
//! The functions below return the raw [`ValidationError`] rather than an
//! [`ObsctlError`](crate::domain::errors::ObsctlError) on purpose. The same
//! failure means different things depending on where it happened: a bad target
//! in `:scene <name>` is a command the user mistyped, while a bad name in the
//! config file is a broken config. Each caller therefore keeps its own
//! mapping — and its own message prefix — and only the rule itself is shared.

use crate::support::validation::{
    MAX_TARGET_TOKEN_LENGTH, ValidationError, trim_and_validate_token_with_max_len,
};

/// Which kind of OBS resource a name, alias, or shortcut belongs to.
///
/// This used to be passed around as a `&str` (`"scene"` / `"audio"`) that was
/// pattern-matched further down to pick a human-readable label. A typo in one
/// of those literals silently fell through to the scene branch and produced a
/// misleading message; an enum cannot be mistyped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Scene,
    AudioInput,
}

impl ResourceKind {
    /// The bare word for this kind, as it appears inside messages such as
    /// `duplicate scene alias: 'main'`.
    pub fn label(self) -> &'static str {
        match self {
            ResourceKind::Scene => "scene",
            ResourceKind::AudioInput => "audio",
        }
    }

    /// How a *name* of this kind is described, as it appears in messages such
    /// as `audio input name must not be blank`.
    pub fn name_label(self) -> &'static str {
        match self {
            ResourceKind::Scene => "scene name",
            ResourceKind::AudioInput => "audio input name",
        }
    }
}

/// The trimmed form of `value`, if `value` is usable as a resource name.
///
/// Preserves case, because a scene called `Main Camera` has to be sent to OBS
/// spelled exactly that way.
pub fn checked_name(value: &str) -> Result<String, ValidationError> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
}

/// [`checked_name`] plus case folding, for comparisons.
///
/// Two names that differ only in case or in surrounding whitespace are the
/// same name as far as matching an alias or spotting a duplicate goes, so
/// every such comparison happens between two `normalized_name` results rather
/// than between the raw strings.
pub fn normalized_name(value: &str) -> Result<String, ValidationError> {
    checked_name(value).map(|value| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_name_trims_but_keeps_case() {
        assert_eq!(checked_name("  Main Camera ").unwrap(), "Main Camera");
    }

    #[test]
    fn normalized_name_folds_case() {
        assert_eq!(normalized_name("  Main Camera ").unwrap(), "main camera");
    }

    #[test]
    fn blank_control_and_oversize_names_are_rejected() {
        assert!(checked_name("   ").is_err());
        assert!(checked_name("main\tcam").is_err());
        assert!(checked_name(&"a".repeat(MAX_TARGET_TOKEN_LENGTH + 1)).is_err());
    }

    #[test]
    fn labels_differ_per_kind() {
        assert_eq!(ResourceKind::Scene.label(), "scene");
        assert_eq!(ResourceKind::AudioInput.label(), "audio");
        assert_eq!(ResourceKind::Scene.name_label(), "scene name");
        assert_eq!(ResourceKind::AudioInput.name_label(), "audio input name");
    }
}
