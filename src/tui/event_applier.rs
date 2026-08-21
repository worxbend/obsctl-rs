use rust_i18n::t;

use crate::{
    ipc::protocol::{LogEvent, ObsEventPayload, ServerMessage, Topic},
    obs::state::ObsSnapshot,
    tui::model::{TuiLogEntry, TuiModel},
};

/// Apply an incoming server message to the model.
/// Returns `true` if an immediate redraw is warranted, `false` for
/// high-frequency events (meters) that the ticker can catch at its
/// normal refresh rate.
pub fn apply_server_message(model: &mut TuiModel, msg: ServerMessage) -> bool {
    let ServerMessage::Event { topic, data } = msg else {
        return true;
    };

    match topic {
        Topic::State => {
            match serde_json::from_value::<ObsSnapshot>(data) {
                Ok(snapshot) => apply_snapshot(model, snapshot),
                // Most `ObsSnapshot` fields have no serde default, so a daemon
                // newer than this TUI lands here. Silently ignoring it left
                // every panel frozen on stale data while the header still said
                // "connected" — a state the user cannot tell from a quiet OBS.
                Err(error) => report(model, "tui.events.undecodable_state", &error),
            }
            true
        }
        Topic::Logs => {
            match serde_json::from_value::<LogEvent>(data) {
                Ok(event) => model.push_log(event.into()),
                Err(error) => report(model, "tui.events.undecodable_log", &error),
            }
            true
        }
        Topic::Events => match serde_json::from_value::<ObsEventPayload>(data) {
            Ok(ObsEventPayload::InputVolumeMeters { inputs }) => {
                for entry in inputs {
                    model.record_meter_level(entry.name, entry.level);
                }
                // Meters arrive many times a second; let the ticker redraw at
                // its normal rate rather than once per reading.
                false
            }
            // A variant this build knows about but does not act on here. Not a
            // problem, and not worth putting in front of the user.
            Ok(_) => {
                tracing::debug!("unhandled TOPIC_EVENTS payload variant");
                true
            }
            Err(error) => {
                report(model, "tui.events.undecodable_event", &error);
                true
            }
        },
    }
}

fn apply_snapshot(model: &mut TuiModel, snapshot: ObsSnapshot) {
    let previous_scene = model.current_scene().map(str::to_string);
    if let (Some(previous), Some(next)) = (previous_scene, snapshot.current_scene.as_deref())
        && previous != next
    {
        model.scene_flash = Some((next.to_string(), model.anim.frame));
    }
    model.snapshot = Some(snapshot);
    model.connected_to_daemon = true;
    model.clamp_cursors();
    model.record_metric_sample();
}

/// Tell the user, in the log pane, that a message from the daemon was dropped.
///
/// Both destinations matter and carry different things: the `tracing` line
/// names the serde error for whoever reads the process log, and the log-pane
/// entry is what stops a stuck dashboard from merely looking idle.
fn report(model: &mut TuiModel, key: &str, error: &serde_json::Error) {
    tracing::warn!("{key}: {error}");
    model.push_log(TuiLogEntry::warning(t!(key, error = error.to_string())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::LogLevel;
    use crate::tui::model::TuiModel;

    fn state_event(data: serde_json::Value) -> ServerMessage {
        ServerMessage::Event {
            topic: Topic::State,
            data,
        }
    }

    /// A snapshot this build cannot parse — a daemon one version ahead, say —
    /// used to be dropped without a word, leaving every panel on stale data
    /// while the header still claimed the daemon was connected.
    #[test]
    fn an_undecodable_snapshot_is_reported_in_the_log_pane() {
        let mut model = TuiModel::default();
        let redraw = apply_server_message(&mut model, state_event(serde_json::json!({})));

        assert!(redraw, "the frame should still be redrawn");
        assert!(
            model.snapshot.is_none(),
            "a snapshot that failed to parse must not be applied"
        );
        let entry = model.logs.last().expect("expected a log entry");
        assert_eq!(entry.level, LogLevel::Warn);
        assert!(
            entry.message.contains("state update"),
            "unexpected message: {}",
            entry.message
        );
    }

    #[test]
    fn a_decodable_snapshot_still_applies_and_marks_the_daemon_connected() {
        let mut model = TuiModel::default();
        let snapshot = serde_json::to_value(crate::obs::state::ObsSnapshot::default()).unwrap();

        assert!(apply_server_message(&mut model, state_event(snapshot)));
        assert!(model.snapshot.is_some());
        assert!(model.connected_to_daemon);
        assert!(model.logs.is_empty(), "a good snapshot must not log");
    }

    #[test]
    fn an_undecodable_obs_event_is_reported_rather_than_ignored() {
        let mut model = TuiModel::default();
        let redraw = apply_server_message(
            &mut model,
            ServerMessage::Event {
                topic: Topic::Events,
                data: serde_json::json!({ "type": "NotAnEventThisBuildKnows" }),
            },
        );

        assert!(redraw);
        let entry = model.logs.last().expect("expected a log entry");
        assert_eq!(entry.level, LogLevel::Warn);
    }
}
