use super::model::TuiModel;

const ALL_COMMANDS: &[&str] = &[
    "/help",
    "/scene",
    "/mute",
    "/unmute",
    "/toggle-mute",
    "/vol",
    "/stream",
    "/rec",
    "/status",
    "/obs-status",
    "/server-status",
    "/reload-config",
    "/dump-config",
    "/validate-config",
    "/reconnect",
    "/quit",
];

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
    if !input.contains(' ') {
        let lower = input.to_ascii_lowercase();
        let mut matches: Vec<String> = ALL_COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(lower.as_str()))
            .map(|s| s.to_string())
            .collect();
        sort_candidates(&mut matches, input);
        return matches;
    }

    let (cmd, arg_prefix) = input.split_once(' ').unwrap();
    let cmd_lower = cmd.to_ascii_lowercase();
    let arg_lower = arg_prefix.to_ascii_lowercase();

    match cmd_lower.as_str() {
        "/scene" | "/set-scene" => {
            let mut candidates: Vec<String> = model
                .scenes()
                .iter()
                .flat_map(|s| {
                    let mut names = vec![s.name.clone()];
                    if let Some(a) = &s.alias {
                        names.push(a.clone());
                    }
                    names
                })
                .filter(|n| n.to_ascii_lowercase().starts_with(arg_lower.as_str()))
                .collect();
            sort_candidates(&mut candidates, arg_prefix);
            candidates
                .into_iter()
                .map(|c| format!("{cmd} {c}"))
                .collect()
        }
        "/mute" | "/unmute" | "/toggle-mute" | "/vol" | "/volume" => {
            let mut candidates: Vec<String> = model
                .audio_inputs()
                .iter()
                .flat_map(|a| {
                    let mut names = vec![a.name.clone()];
                    if let Some(alias) = &a.alias {
                        names.push(alias.clone());
                    }
                    names
                })
                .filter(|n| n.to_ascii_lowercase().starts_with(arg_lower.as_str()))
                .collect();
            sort_candidates(&mut candidates, arg_prefix);
            candidates
                .into_iter()
                .map(|c| format!("{cmd} {c}"))
                .collect()
        }
        _ => vec![],
    }
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
        model.snapshot = Some(ObsSnapshot {
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

    #[test]
    fn command_prefix_no_space() {
        let model = make_model(vec![], vec![]);
        let result = compute("/sc", &model);
        assert_eq!(result, vec!["/scene".to_string()]);
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
