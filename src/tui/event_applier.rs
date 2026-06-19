use crate::{
    ipc::protocol::{LogEvent, ServerMessage, TOPIC_LOGS, TOPIC_STATE},
    obs::state::ObsSnapshot,
    tui::model::TuiModel,
};

pub fn apply_server_message(model: &mut TuiModel, msg: ServerMessage) {
    if let ServerMessage::Event { topic, data } = msg {
        match topic.as_str() {
            TOPIC_STATE => {
                if let Ok(snapshot) = serde_json::from_value::<ObsSnapshot>(data) {
                    model.snapshot = Some(snapshot);
                    model.connected_to_daemon = true;
                }
            }
            TOPIC_LOGS => {
                if let Ok(event) = serde_json::from_value::<LogEvent>(data) {
                    model.push_log(event.into());
                }
            }
            _ => {}
        }
    }
}
