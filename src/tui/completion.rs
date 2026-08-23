use super::model::TuiModel;
use crate::domain::parser::{CANONICAL_PALETTE_COMMANDS, PALETTE_PREFIXES};

/// Split a leading palette prefix off `input`, if it has one.
fn split_prefix(input: &str) -> (&str, &str) {
    match input.chars().next() {
        Some(c) if PALETTE_PREFIXES.contains(&c) => input.split_at(c.len_utf8()),
        _ => ("", input),
    }
}

fn sort_candidates(candidates: &mut Vec<String>, prefix: &str) {
    let prefix_lower = prefix.to_ascii_lowercase();
    candidates.sort_by_cached_key(|candidate| {
        let candidate_lower = candidate.to_ascii_lowercase();
        (
            candidate_lower != prefix_lower,
            candidate_lower,
            candidate.clone(),
        )
    });
    candidates.dedup();
}

pub fn compute(input: &str, model: &TuiModel) -> Vec<String> {
    let (prefix, body) = split_prefix(input);

    if !body.contains(' ') {
        let lower = body.to_ascii_lowercase();
        let mut matches: Vec<String> = CANONICAL_PALETTE_COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(lower.as_str()))
            .map(|cmd| format!("{prefix}{cmd}"))
            .collect();
        sort_candidates(&mut matches, input);
        return matches;
    }

    let (typed_cmd, raw_arg_prefix) = match body.split_once(' ') {
        Some(parts) => parts,
        None => return vec![],
    };
    let typed_cmd = typed_cmd.trim_end();
    // Candidates echo back exactly what the user typed, prefix and casing
    // included; only the lookup key is normalized.
    let cmd = format!("{prefix}{typed_cmd}");
    let cmd_key = typed_cmd.to_ascii_lowercase();
    let arg_prefix = raw_arg_prefix.trim_start();
    let arg_lower = arg_prefix.to_ascii_lowercase();

    // Each arm answers only "what could the user be naming here?"; the
    // filtering, sorting, and re-attaching of the command word are the same
    // for all of them and happen once, below.
    let pool: Vec<String> = match cmd_key.as_str() {
        "scene" | "set-scene" => model
            .scenes()
            .iter()
            .flat_map(|s| name_and_alias(&s.name, &s.alias))
            .collect(),
        "profile" | "set-profile" => model.profiles().to_vec(),
        // The obsctl scene profiles, which the daemon publishes in the same
        // snapshot as the OBS profiles above and which are a different thing.
        "scene-profile" | "set-scene-profile" => model.scene_profile_names(),
        "collection" | "set-collection" | "scene-collection" => model.scene_collections().to_vec(),
        "mute" | "unmute" | "toggle-mute" | "vol" | "volume" => model
            .audio_inputs()
            .iter()
            .flat_map(|a| name_and_alias(&a.name, &a.alias))
            .collect(),
        _ => return vec![],
    };

    let mut candidates: Vec<String> = pool
        .into_iter()
        .filter(|n| n.to_ascii_lowercase().starts_with(arg_lower.as_str()))
        .collect();
    sort_candidates(&mut candidates, arg_prefix);
    candidates
        .into_iter()
        .map(|c| format!("{cmd} {c}"))
        .collect()
}

/// Both spellings the user may type for one target: the name OBS uses, and
/// the alias the config gave it, if any.
fn name_and_alias(name: &str, alias: &Option<String>) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if let Some(alias) = alias {
        names.push(alias.clone());
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        obs::state::{AudioState, ObsSnapshot, SceneState},
        tui::model::TuiModel,
    };

    fn make_model(scenes: Vec<SceneState>, audio: Vec<AudioState>) -> TuiModel {
        let mut model = TuiModel::default();
        model.set_snapshot(ObsSnapshot {
            scenes,
            audio_inputs: audio,
            ..Default::default()
        });
        model.clamp_cursors();
        model
    }

    fn scene(name: &str, alias: Option<&str>) -> SceneState {
        SceneState {
            name: name.to_string(),
            alias: alias.map(|a| a.to_string()),
            ..Default::default()
        }
    }

    fn audio(name: &str, alias: Option<&str>) -> AudioState {
        AudioState {
            name: name.to_string(),
            alias: alias.map(|a| a.to_string()),
            ..Default::default()
        }
    }

    /// Three commands start with `sc`, and all three are offered.
    #[test]
    fn command_prefix_no_space() {
        let model = make_model(vec![], vec![]);
        let result = compute("/sc", &model);
        assert_eq!(
            result,
            vec![
                "/scene".to_string(),
                "/scene-profile".to_string(),
                "/scene-profile-off".to_string(),
            ]
        );
    }

    #[test]
    fn candidates_keep_whichever_palette_prefix_was_typed() {
        let model = make_model(vec![scene("main", None)], vec![]);
        assert_eq!(
            compute(":sc", &model),
            vec![
                ":scene".to_string(),
                ":scene-profile".to_string(),
                ":scene-profile-off".to_string(),
            ]
        );
        assert_eq!(compute(":scene m", &model), vec![":scene main".to_string()]);
        // No prefix at all is still completed, unprefixed.
        assert_eq!(
            compute("sc", &model),
            vec![
                "scene".to_string(),
                "scene-profile".to_string(),
                "scene-profile-off".to_string(),
            ]
        );
    }

    #[test]
    fn help_command_prefix() {
        let model = make_model(vec![], vec![]);
        let result = compute("/h", &model);
        assert_eq!(result, vec!["/help".to_string()]);
    }

    #[test]
    fn status_exact_prefix() {
        let model = make_model(vec![], vec![]);
        let result = compute("/status", &model);
        assert_eq!(result, vec!["/status".to_string()]);
    }

    #[test]
    fn exact_command_match_sorts_before_other_prefix_matches() {
        let model = make_model(vec![], vec![]);
        let result = compute("/REC", &model);
        assert_eq!(result, vec!["/rec".to_string(), "/reconnect".to_string()]);
    }

    #[test]
    fn scene_arg_filter() {
        let model = make_model(
            vec![
                scene("main", None),
                scene("media", Some("m2")),
                scene("overlay", None),
            ],
            vec![],
        );
        let result = compute("/scene m", &model);
        assert!(result.contains(&"/scene main".to_string()));
        assert!(result.contains(&"/scene media".to_string()));
        assert!(result.contains(&"/scene m2".to_string()));
        assert!(!result.iter().any(|s| s.contains("overlay")));
    }

    #[test]
    fn profile_arg_filter() {
        let mut model = make_model(vec![], vec![]);
        model.set_snapshot(ObsSnapshot {
            profiles: vec!["Default".to_string(), "Streaming".to_string()],
            ..Default::default()
        });
        let result = compute("/profile S", &model);
        assert!(result.contains(&"/profile Streaming".to_string()));
        assert!(!result.iter().any(|s| s.contains("Default")));
    }

    #[test]
    fn collection_arg_filter() {
        let mut model = make_model(vec![], vec![]);
        model.set_snapshot(ObsSnapshot {
            scene_collections: vec!["Podcast".to_string(), "Gaming".to_string()],
            ..Default::default()
        });
        let result = compute("/collection G", &model);
        assert!(result.contains(&"/collection Gaming".to_string()));
        assert!(!result.iter().any(|s| s.contains("Podcast")));
    }

    #[test]
    fn scene_arg_command_match_is_case_insensitive_and_preserves_typed_command() {
        let model = make_model(
            vec![scene("main", None), scene("media", Some("m2"))],
            vec![],
        );
        let result = compute("/SCENE m", &model);
        assert!(result.contains(&"/SCENE main".to_string()));
        assert!(result.contains(&"/SCENE media".to_string()));
        assert!(result.contains(&"/SCENE m2".to_string()));
        assert!(result.iter().all(|s| s.starts_with("/SCENE ")));
    }

    #[test]
    fn set_scene_alias_arg_filter_preserves_typed_command() {
        let model = make_model(
            vec![scene("main", None), scene("media", Some("m2"))],
            vec![],
        );
        let result = compute("/set-scene m", &model);
        assert!(result.contains(&"/set-scene main".to_string()));
        assert!(result.contains(&"/set-scene media".to_string()));
        assert!(result.contains(&"/set-scene m2".to_string()));
        assert!(result.iter().all(|s| s.starts_with("/set-scene ")));
    }

    #[test]
    fn mute_arg_filter() {
        let model = make_model(
            vec![],
            vec![
                audio("mic", None),
                audio("music", Some("m-bg")),
                audio("desktop", None),
            ],
        );
        let result = compute("/mute m", &model);
        assert!(result.contains(&"/mute mic".to_string()));
        assert!(result.contains(&"/mute music".to_string()));
        assert!(result.contains(&"/mute m-bg".to_string()));
        assert!(!result.iter().any(|s| s.contains("desktop")));
    }

    #[test]
    fn audio_arg_command_match_is_case_insensitive_and_preserves_typed_command() {
        let model = make_model(vec![], vec![audio("mic", None), audio("Mic Aux", None)]);
        let result = compute("/MUTE mic", &model);
        assert_eq!(
            result,
            vec!["/MUTE mic".to_string(), "/MUTE Mic Aux".to_string()]
        );
    }

    #[test]
    fn volume_alias_arg_filter_preserves_typed_command() {
        let model = make_model(vec![], vec![audio("mic", None), audio("Mic Aux", None)]);
        let result = compute("/volume mic", &model);
        assert_eq!(
            result,
            vec!["/volume mic".to_string(), "/volume Mic Aux".to_string()]
        );
    }

    #[test]
    fn exact_arg_match_sorts_before_other_prefix_matches() {
        let model = make_model(
            vec![],
            vec![
                audio("Mic Aux", None),
                audio("mIc", None),
                audio("music", None),
            ],
        );
        let result = compute("/mute mic", &model);
        assert_eq!(
            result,
            vec!["/mute mIc".to_string(), "/mute Mic Aux".to_string()]
        );
    }

    #[test]
    fn unknown_cmd_with_arg_returns_empty() {
        let model = make_model(vec![], vec![]);
        let result = compute("/stream something", &model);
        assert!(result.is_empty());
    }
}
