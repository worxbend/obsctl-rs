use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem},
};

use crate::{
    ipc::protocol::LogLevel,
    tui::{
        model::{TuiLogEntry, TuiModel},
        theme::Theme,
        widgets::chrome,
    },
};

pub fn render(f: &mut Frame, area: Rect, model: &TuiModel) {
    let height = area.height.saturating_sub(2) as usize;
    let skip = model.logs.len().saturating_sub(height);
    let resources = known_resources(model);

    let items: Vec<ListItem> = model
        .logs
        .iter()
        .skip(skip)
        .map(|entry| ListItem::new(log_line(entry, model, &resources)))
        .collect();

    let block = chrome::panel(
        model.symbol("📡", "L"),
        "Logs // Event Stream",
        "live daemon feed",
        model.logs.len(),
        false,
        model,
    );
    f.render_widget(List::new(items).block(block), area);
}

fn log_line(
    entry: &TuiLogEntry,
    model: &TuiModel,
    resources: &[(&str, ResourceKind)],
) -> Line<'static> {
    let theme = model.theme;
    let time = format!(
        "{:02}:{:02}:{:02}",
        entry.timestamp.hour(),
        entry.timestamp.minute(),
        entry.timestamp.second()
    );
    let marker = match entry.level {
        LogLevel::Trace => model.symbol("·", "."),
        LogLevel::Debug => model.symbol("◦", "."),
        LogLevel::Info => model.symbol("●", "i"),
        LogLevel::Warn => model.symbol("▲", "!"),
        LogLevel::Error => model.symbol("◆", "x"),
    };
    let style = style_for_level(entry.level, theme);
    let mut spans = vec![
        Span::styled(format!(" {marker} "), style),
        Span::styled(time, Style::default().fg(theme.muted)),
        Span::styled(
            format!(" {:<5} ", level_label(entry.level)),
            style.add_modifier(ratatui::style::Modifier::BOLD),
        ),
    ];
    if let Some(target) = entry.target.as_deref().filter(|target| !target.is_empty()) {
        spans.push(Span::styled(
            format!("{target}  "),
            Style::default().fg(theme.accent_alt),
        ));
    }
    spans.extend(highlight_message(&entry.message, model, resources));
    Line::from(spans)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceKind {
    Scene,
    Audio,
    Profile,
    Collection,
}

fn highlight_message(
    message: &str,
    model: &TuiModel,
    resources: &[(&str, ResourceKind)],
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut cursor = 0;

    while cursor < message.len() {
        let rest = &message[cursor..];

        if let Some((name, kind)) = resources
            .iter()
            .find(|(name, _)| resource_matches(rest, name))
        {
            spans.push(Span::styled(
                rest[..name.len()].to_string(),
                resource_style(*kind, model.theme),
            ));
            cursor += name.len();
            continue;
        }

        let Some(ch) = rest.chars().next() else {
            break;
        };
        if matches!(ch, '\'' | '"' | '`') {
            let delimiter_len = ch.len_utf8();
            let closing = rest[delimiter_len..]
                .find(ch)
                .map(|offset| delimiter_len + offset + delimiter_len)
                .unwrap_or(rest.len());
            let quoted = &rest[..closing];
            let inner = quoted
                .strip_prefix(ch)
                .and_then(|value| value.strip_suffix(ch));
            let style = inner
                .and_then(|value| exact_resource_kind(value, resources))
                .map(|kind| resource_style(kind, model.theme))
                .unwrap_or_else(|| {
                    Style::default()
                        .fg(model.theme.accent_alt)
                        .add_modifier(Modifier::ITALIC)
                });
            spans.push(Span::styled(quoted.to_string(), style));
            cursor += closing;
            continue;
        }

        if rest.starts_with("->") || rest.starts_with("=>") {
            spans.push(Span::styled(
                rest[..2].to_string(),
                Style::default()
                    .fg(model.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            cursor += 2;
            continue;
        }

        if is_token_char(ch) {
            let token_len = rest
                .char_indices()
                .find(|(_, candidate)| !is_token_char(*candidate))
                .map(|(index, _)| index)
                .unwrap_or(rest.len());
            let token = &rest[..token_len];
            spans.push(Span::styled(
                token.to_string(),
                token_style(token, model.theme),
            ));
            cursor += token_len;
            continue;
        }

        spans.push(Span::styled(
            ch.to_string(),
            Style::default().fg(model.theme.fg),
        ));
        cursor += ch.len_utf8();
    }

    spans
}

fn known_resources(model: &TuiModel) -> Vec<(&str, ResourceKind)> {
    let mut resources = Vec::new();
    if let Some(snapshot) = model.snapshot.as_ref() {
        for scene in &snapshot.scenes {
            resources.push((scene.name.as_str(), ResourceKind::Scene));
            if let Some(alias) = scene.alias.as_deref() {
                resources.push((alias, ResourceKind::Scene));
            }
        }
        for input in &snapshot.audio_inputs {
            resources.push((input.name.as_str(), ResourceKind::Audio));
            if let Some(alias) = input.alias.as_deref() {
                resources.push((alias, ResourceKind::Audio));
            }
        }
    }
    resources.extend(
        model
            .profiles()
            .iter()
            .map(|profile| (profile.as_str(), ResourceKind::Profile)),
    );
    resources.extend(
        model
            .scene_collections()
            .iter()
            .map(|collection| (collection.as_str(), ResourceKind::Collection)),
    );
    resources.retain(|(name, _)| !name.is_empty());
    resources.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    resources.dedup_by(|left, right| left.0.eq_ignore_ascii_case(right.0));
    resources
}

fn resource_matches(candidate: &str, name: &str) -> bool {
    let Some(prefix) = candidate.get(..name.len()) else {
        return false;
    };
    if !prefix.eq_ignore_ascii_case(name) {
        return false;
    }
    candidate[name.len()..]
        .chars()
        .next()
        .is_none_or(|ch| !is_boundary_word_char(ch))
}

fn exact_resource_kind(
    candidate: &str,
    resources: &[(&str, ResourceKind)],
) -> Option<ResourceKind> {
    resources
        .iter()
        .find(|(name, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, kind)| *kind)
}

fn is_boundary_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn is_token_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/' | ':' | '.' | '%' | '#')
}

fn resource_style(kind: ResourceKind, theme: Theme) -> Style {
    let color = match kind {
        ResourceKind::Scene => theme.accent,
        ResourceKind::Audio => theme.info,
        ResourceKind::Profile => theme.warning,
        ResourceKind::Collection => theme.accent_alt,
    };
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

fn token_style(token: &str, theme: Theme) -> Style {
    let normalized = token.trim_matches(|ch: char| matches!(ch, '.' | ':' | '/'));
    let color = if token.starts_with('/') {
        Some(theme.accent)
    } else if is_danger_keyword(normalized) {
        Some(theme.danger)
    } else if is_warning_keyword(normalized) {
        Some(theme.warning)
    } else if is_success_keyword(normalized) {
        Some(theme.success)
    } else if is_subject_keyword(normalized) {
        Some(theme.accent_alt)
    } else if token.chars().any(|ch| ch.is_ascii_digit()) {
        Some(theme.info)
    } else {
        None
    };
    color.map_or_else(
        || Style::default().fg(theme.fg),
        |color| Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn is_danger_keyword(word: &str) -> bool {
    matches_ci(
        word,
        &[
            "error",
            "failed",
            "failure",
            "disconnected",
            "unavailable",
            "denied",
            "invalid",
            "malformed",
            "timeout",
            "dropped",
            "closed",
            "fatal",
        ],
    )
}

fn is_warning_keyword(word: &str) -> bool {
    matches_ci(
        word,
        &[
            "warning",
            "warn",
            "retry",
            "reconnect",
            "reconnecting",
            "muted",
            "stopped",
            "shutdown",
            "stale",
        ],
    )
}

fn is_success_keyword(word: &str) -> bool {
    matches_ci(
        word,
        &[
            "connected",
            "ready",
            "started",
            "active",
            "switched",
            "reloaded",
            "success",
            "enabled",
            "unmuted",
            "streaming",
            "recording",
            "complete",
        ],
    )
}

fn is_subject_keyword(word: &str) -> bool {
    matches_ci(
        word,
        &[
            "obs",
            "scene",
            "profile",
            "collection",
            "audio",
            "input",
            "source",
            "stream",
            "record",
            "config",
            "daemon",
            "server",
            "command",
            "websocket",
        ],
    )
}

fn matches_ci(candidate: &str, values: &[&str]) -> bool {
    values
        .iter()
        .any(|value| candidate.eq_ignore_ascii_case(value))
}

fn style_for_level(level: LogLevel, theme: Theme) -> Style {
    match level {
        LogLevel::Trace => Style::default().fg(theme.muted),
        LogLevel::Debug => Style::default().fg(theme.muted),
        LogLevel::Info => Style::default().fg(theme.fg),
        LogLevel::Warn => Style::default().fg(theme.warning),
        LogLevel::Error => Style::default().fg(theme.danger),
    }
}

fn level_label(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "TRACE",
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::state::{AudioState, ObsSnapshot, SceneState};

    fn semantic_model() -> TuiModel {
        let mut model = TuiModel::default();
        model.snapshot = Some(ObsSnapshot {
            scenes: vec![SceneState {
                name: "Main Scene".into(),
                alias: Some("main".into()),
                ..SceneState::default()
            }],
            audio_inputs: vec![AudioState {
                name: "Desktop Audio".into(),
                alias: Some("desk".into()),
                ..AudioState::default()
            }],
            profiles: vec!["Streaming".into()],
            scene_collections: vec!["Podcast".into()],
            ..ObsSnapshot::default()
        });
        model.clamp_cursors();
        model
    }

    fn span_for<'a>(spans: &'a [Span<'static>], content: &str) -> &'a Span<'static> {
        spans
            .iter()
            .find(|span| span.content == content)
            .unwrap_or_else(|| panic!("missing highlighted span {content:?} in {spans:?}"))
    }

    #[test]
    fn highlights_resources_keywords_commands_and_numbers() {
        let model = semantic_model();
        let resources = known_resources(&model);
        let message =
            "scene Main Scene switched -> profile Streaming via /scene in 42ms; OBS connected";
        let spans = highlight_message(message, &model, &resources);
        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            message
        );
        assert_eq!(
            span_for(&spans, "Main Scene").style.fg,
            Some(model.theme.accent)
        );
        assert_eq!(
            span_for(&spans, "Streaming").style.fg,
            Some(model.theme.warning)
        );
        assert_eq!(
            span_for(&spans, "switched").style.fg,
            Some(model.theme.success)
        );
        assert_eq!(
            span_for(&spans, "/scene").style.fg,
            Some(model.theme.accent)
        );
        assert_eq!(span_for(&spans, "42ms").style.fg, Some(model.theme.info));
        assert_eq!(
            span_for(&spans, "OBS").style.fg,
            Some(model.theme.accent_alt)
        );
    }

    #[test]
    fn highlights_quoted_resource_names_and_error_terms() {
        let model = semantic_model();
        let resources = known_resources(&model);
        let spans = highlight_message(
            "failed to switch scene 'Main Scene': timeout on Desktop Audio",
            &model,
            &resources,
        );
        assert_eq!(
            span_for(&spans, "failed").style.fg,
            Some(model.theme.danger)
        );
        assert_eq!(
            span_for(&spans, "'Main Scene'").style.fg,
            Some(model.theme.accent)
        );
        assert_eq!(
            span_for(&spans, "timeout").style.fg,
            Some(model.theme.danger)
        );
        assert_eq!(
            span_for(&spans, "Desktop Audio").style.fg,
            Some(model.theme.info)
        );
    }

    #[test]
    fn resource_matching_respects_word_boundaries() {
        let model = semantic_model();
        let resources = known_resources(&model);
        let spans = highlight_message("Mainframe and Sceneography", &model, &resources);
        assert_eq!(spans[0].content, "Mainframe");
        assert_eq!(spans[0].style.fg, Some(model.theme.fg));
        assert!(spans.iter().all(|span| span.content != "Main Scene"));
    }
}
