use crate::{
    ipc::protocol::{LogEvent, ObsEventPayload, ServerMessage, TOPIC_EVENTS, TOPIC_LOGS, TOPIC_STATE},
    obs::state::ObsSnapshot,
    tui::model::TuiModel,
};

/// Apply an incoming server message to the model.
/// Returns `true` if an immediate redraw is warranted, `false` for
/// high-frequency events (meters) that the ticker can catch at its
/// normal refresh rate.
pub fn apply_server_message(model: &mut TuiModel, msg: ServerMessage) -> bool {
    if let ServerMessage::Event { topic, data } = msg {
        match topic.as_str() {
            TOPIC_STATE => {
                if let Ok(snapshot) = serde_json::from_value::<ObsSnapshot>(data) {
                    model.snapshot = Some(snapshot);
                    model.connected_to_daemon = true;
                    model.clamp_cursors();
                }
                return true;
            }
            TOPIC_LOGS => {
                if let Ok(event) = serde_json::from_value::<LogEvent>(data) {
                    model.push_log(event.into());
                }
                return true;
            }
            TOPIC_EVENTS => {
                if let Ok(ObsEventPayload::InputVolumeMeters { inputs }) =
                    serde_json::from_value::<ObsEventPayload>(data)
                {
                    for entry in inputs {
                        model.meter_levels.insert(entry.name, entry.level);
                    }
                    return false; // let the ticker redraw at normal rate
                }
                return true;
            }
            _ => {}
        }
    }
    true
}
