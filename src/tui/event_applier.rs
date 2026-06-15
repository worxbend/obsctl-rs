use crate::{ipc::protocol::ServerMessage, obs::state::ObsSnapshot, tui::model::TuiModel};

pub fn apply_server_message(model: &mut TuiModel, msg: ServerMessage) {
    if let ServerMessage::Event { topic, data } = msg {
        match topic.as_str() {
            "state" => {
                if let Ok(snapshot) = serde_json::from_value::<ObsSnapshot>(data) {
                    model.snapshot = Some(snapshot);
                    model.connected_to_daemon = true;
                }
            }
            "logs" => {
                let text = if let Some(msg) = data.get("message").and_then(|m| m.as_str()) {
                    msg.to_string()
                } else {
                    data.to_string()
                };
                model.logs.push(text);
                if model.logs.len() > 200 {
                    model.logs.remove(0);
                }
            }
            _ => {}
        }
    }
}
