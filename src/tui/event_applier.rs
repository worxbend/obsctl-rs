use crate::{
    ipc::protocol::{LogEvent, ObsEventPayload, ServerMessage, Topic},
    obs::state::ObsSnapshot,
    tui::model::TuiModel,
};

/// Apply an incoming server message to the model.
/// Returns `true` if an immediate redraw is warranted, `false` for
/// high-frequency events (meters) that the ticker can catch at its
/// normal refresh rate.
pub fn apply_server_message(model: &mut TuiModel, msg: ServerMessage) -> bool {
    if let ServerMessage::Event { topic, data } = msg {
        match topic {
            Topic::State => {
                if let Ok(snapshot) = serde_json::from_value::<ObsSnapshot>(data) {
                    let previous_scene = model.current_scene().map(str::to_string);
                    if let (Some(previous), Some(next)) =
                        (previous_scene, snapshot.current_scene.as_deref())
                        && previous != next
                    {
                        model.scene_flash = Some((next.to_string(), model.anim.frame));
                    }
                    model.snapshot = Some(snapshot);
                    model.connected_to_daemon = true;
                    model.clamp_cursors();
                    model.record_metric_sample();
                }
                return true;
            }
            Topic::Logs => {
                if let Ok(event) = serde_json::from_value::<LogEvent>(data) {
                    model.push_log(event.into());
                }
                return true;
            }
            Topic::Events => {
                match serde_json::from_value::<ObsEventPayload>(data) {
                    Ok(ObsEventPayload::InputVolumeMeters { inputs }) => {
                        for entry in inputs {
                            model.record_meter_level(entry.name, entry.level);
                        }
                        return false; // let the ticker redraw at normal rate
                    }
                    _ => {
                        tracing::debug!("unhandled TOPIC_EVENTS payload variant");
                    }
                }
                return true;
            }
        }
    }
    true
}
