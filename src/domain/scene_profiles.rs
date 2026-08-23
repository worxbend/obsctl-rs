//! Which scenes are hidden from the scene list right now.
//!
//! OBS setups often contain "utility" scenes that only exist to be nested
//! inside other scenes. Listing them next to the scenes a user actually
//! switches to is noise, so obsctl lets them be hidden — either permanently,
//! with the per-scene `hidden` flag in the config, or through a named
//! **scene profile**: a saved set of visibility choices that can be switched
//! as a unit. (A scene profile is an obsctl concept and has nothing to do with
//! an OBS profile.)
//!
//! Two sources of truth for the same question is how a UI ends up disagreeing
//! with the daemon about what should be on screen, so there is exactly one
//! answer here: build a [`SceneVisibility`] from the config once, then ask it.
//! Nothing else may decide whether a scene is hidden.
//!
//! This module deliberately imports nothing outside `domain` — no `config`, no
//! `obs`, no `ipc`. The rule it encodes is about names and booleans, not about
//! any particular wire type, and keeping it that way is what lets the config
//! layer, the daemon, and the TUI all share it.

use std::collections::HashSet;

use crate::domain::names::normalized_name;

/// Which scenes are hidden right now, resolved once from the config.
///
/// Cheap to clone and to query; a resolved value is passed around rather than
/// re-derived, so every consumer within one snapshot sees the same answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SceneVisibility {
    /// Normalized (trimmed, lowercased) names of the hidden scenes.
    hidden: HashSet<String>,
}

/// The active scene profile's hidden list, or the absence of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveSceneProfile<'a> {
    /// No profile selected, or the selected one does not exist: the
    /// per-scene `hidden` flags are the answer.
    None,
    /// A profile is active: its list is the whole answer.
    Named(&'a [String]),
}

impl SceneVisibility {
    /// Resolve the hidden set from the config's two inputs.
    ///
    /// `baseline` is (scene name, that scene's `hidden` flag) for every
    /// configured scene. Names that do not survive [`normalized_name`] are
    /// dropped rather than rejected — validation already reported them, and a
    /// single unusable entry must not decide the visibility of every other
    /// scene.
    ///
    /// An active profile **replaces** the baseline instead of adding to it.
    /// That is what makes a profile able to reveal a scene that `scenes:`
    /// marks hidden, and what makes "show everything" expressible as a profile
    /// with an empty list. A union could do neither.
    pub fn resolve<'a>(
        baseline: impl IntoIterator<Item = (&'a str, bool)>,
        active: ActiveSceneProfile<'_>,
    ) -> Self {
        let hidden = match active {
            ActiveSceneProfile::None => baseline
                .into_iter()
                .filter(|(_, hidden)| *hidden)
                .filter_map(|(name, _)| normalized_name(name).ok())
                .collect(),
            ActiveSceneProfile::Named(list) => list
                .iter()
                .filter_map(|name| normalized_name(name).ok())
                .collect(),
        };

        Self { hidden }
    }

    /// Whether `scene_name` — spelled however OBS spells it — is hidden.
    ///
    /// A name that is not usable as a resource name cannot have been put in
    /// the hidden set, so it is reported visible rather than treated as an
    /// error: showing an oddly named scene is the recoverable outcome.
    pub fn is_hidden(&self, scene_name: &str) -> bool {
        normalized_name(scene_name)
            .map(|name| self.hidden.contains(&name))
            .unwrap_or(false)
    }

    /// How many distinct scenes the resolved set hides, for a caller that has
    /// to show that count.
    pub fn hidden_count(&self) -> usize {
        self.hidden.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn baseline() -> Vec<(&'static str, bool)> {
        vec![
            ("Main", false),
            ("Utility BG", true),
            ("Overlay Src", false),
        ]
    }

    #[test]
    fn without_a_profile_the_per_scene_flags_decide() {
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::None);
        assert!(visibility.is_hidden("Utility BG"));
        assert!(!visibility.is_hidden("Main"));
        assert_eq!(visibility.hidden_count(), 1);
    }

    /// The replacement rule, in both directions: the profile hides a scene the
    /// baseline shows, and shows a scene the baseline hides.
    #[test]
    fn an_active_profile_replaces_the_baseline() {
        let profile = vec!["Overlay Src".to_string()];
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::Named(&profile));
        assert!(visibility.is_hidden("Overlay Src"));
        assert!(!visibility.is_hidden("Utility BG"));
        assert_eq!(visibility.hidden_count(), 1);
    }

    /// A profile naming nothing is how "show every scene" is expressed.
    #[test]
    fn an_empty_profile_hides_nothing() {
        let profile: Vec<String> = Vec::new();
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::Named(&profile));
        assert!(!visibility.is_hidden("Utility BG"));
        assert_eq!(visibility.hidden_count(), 0);
    }

    /// A config naming a profile that no longer exists resolves to `None` at
    /// the config layer, which lands here as the baseline answer.
    #[test]
    fn an_unknown_profile_falls_back_to_the_baseline() {
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::None);
        assert!(visibility.is_hidden("Utility BG"));
    }

    #[test]
    fn matching_ignores_case_and_surrounding_whitespace() {
        let profile = vec!["  utility bg  ".to_string()];
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::Named(&profile));
        assert!(visibility.is_hidden("Utility BG"));
        assert!(visibility.is_hidden(" UTILITY BG "));
    }

    #[test]
    fn unusable_names_are_dropped_rather_than_hiding_everything() {
        let profile = vec!["bad\nname".to_string(), "Utility BG".to_string()];
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::Named(&profile));
        assert_eq!(visibility.hidden_count(), 1);
        assert!(visibility.is_hidden("Utility BG"));
        assert!(!visibility.is_hidden("bad\nname"));
    }

    #[test]
    fn the_same_scene_named_twice_counts_once() {
        let profile = vec!["Utility BG".to_string(), "utility bg".to_string()];
        let visibility = SceneVisibility::resolve(baseline(), ActiveSceneProfile::Named(&profile));
        assert_eq!(visibility.hidden_count(), 1);
    }
}
