use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::obs::protocol::RequestData;

static OBS_REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_id() -> String {
    let n = OBS_REQ_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("obs-{n:06}")
}

pub fn get_version() -> RequestData {
    RequestData {
        request_type: "GetVersion".to_string(),
        request_id: next_id(),
        request_data: None,
    }
}

pub fn get_scene_list() -> RequestData {
    RequestData {
        request_type: "GetSceneList".to_string(),
        request_id: next_id(),
        request_data: None,
    }
}

pub fn get_current_program_scene() -> RequestData {
    RequestData {
        request_type: "GetCurrentProgramScene".to_string(),
        request_id: next_id(),
        request_data: None,
    }
}

pub fn set_current_program_scene(scene_name: &str) -> RequestData {
    RequestData {
        request_type: "SetCurrentProgramScene".to_string(),
        request_id: next_id(),
        request_data: Some(json!({ "sceneName": scene_name })),
    }
}

pub fn get_input_list() -> RequestData {
    RequestData {
        request_type: "GetInputList".to_string(),
        request_id: next_id(),
        request_data: None,
    }
}

pub fn get_input_mute(input_name: &str) -> RequestData {
    RequestData {
        request_type: "GetInputMute".to_string(),
        request_id: next_id(),
        request_data: Some(json!({ "inputName": input_name })),
    }
}

pub fn set_input_mute(input_name: &str, muted: bool) -> RequestData {
    RequestData {
        request_type: "SetInputMute".to_string(),
        request_id: next_id(),
        request_data: Some(json!({ "inputName": input_name, "inputMuted": muted })),
    }
}

pub fn toggle_input_mute(input_name: &str) -> RequestData {
    RequestData {
        request_type: "ToggleInputMute".to_string(),
        request_id: next_id(),
        request_data: Some(json!({ "inputName": input_name })),
    }
}

pub fn get_input_volume(input_name: &str) -> RequestData {
    RequestData {
        request_type: "GetInputVolume".to_string(),
        request_id: next_id(),
        request_data: Some(json!({ "inputName": input_name })),
    }
}

pub fn set_input_volume(input_name: &str, volume_mul: f64) -> RequestData {
    RequestData {
        request_type: "SetInputVolume".to_string(),
        request_id: next_id(),
        request_data: Some(json!({ "inputName": input_name, "inputVolumeMul": volume_mul })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_unique() {
        let a = get_version();
        let b = get_version();
        assert_ne!(a.request_id, b.request_id);
    }

    #[test]
    fn set_scene_includes_name() {
        let r = set_current_program_scene("Main Scene");
        let data = r.request_data.unwrap();
        assert_eq!(data["sceneName"], "Main Scene");
    }

    #[test]
    fn set_volume_includes_mul() {
        let r = set_input_volume("Mic", 0.5);
        let data = r.request_data.unwrap();
        assert!((data["inputVolumeMul"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    }
}
