use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::obs::protocol::RequestData;

static OBS_REQ_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_id() -> String {
    let n = OBS_REQ_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("obs-{n:06}")
}

fn req(type_: &str) -> RequestData {
    RequestData {
        request_type: type_.to_string(),
        request_id: next_id(),
        request_data: None,
    }
}

fn req_with(type_: &str, data: serde_json::Value) -> RequestData {
    RequestData {
        request_type: type_.to_string(),
        request_id: next_id(),
        request_data: Some(data),
    }
}

pub fn get_version() -> RequestData {
    req("GetVersion")
}

pub fn get_scene_list() -> RequestData {
    req("GetSceneList")
}

pub fn get_current_program_scene() -> RequestData {
    req("GetCurrentProgramScene")
}

pub fn set_current_program_scene(scene_name: &str) -> RequestData {
    req_with("SetCurrentProgramScene", json!({ "sceneName": scene_name }))
}

pub fn get_input_list() -> RequestData {
    req("GetInputList")
}

pub fn get_input_mute(input_name: &str) -> RequestData {
    req_with("GetInputMute", json!({ "inputName": input_name }))
}

pub fn set_input_mute(input_name: &str, muted: bool) -> RequestData {
    req_with(
        "SetInputMute",
        json!({ "inputName": input_name, "inputMuted": muted }),
    )
}

pub fn toggle_input_mute(input_name: &str) -> RequestData {
    req_with("ToggleInputMute", json!({ "inputName": input_name }))
}

pub fn get_input_volume(input_name: &str) -> RequestData {
    req_with("GetInputVolume", json!({ "inputName": input_name }))
}

pub fn set_input_volume(input_name: &str, volume_mul: f64) -> RequestData {
    req_with(
        "SetInputVolume",
        json!({ "inputName": input_name, "inputVolumeMul": volume_mul }),
    )
}

pub fn get_stream_status() -> RequestData {
    req("GetStreamStatus")
}

pub fn get_record_status() -> RequestData {
    req("GetRecordStatus")
}

pub fn toggle_stream() -> RequestData {
    req("ToggleStream")
}

pub fn toggle_record() -> RequestData {
    req("ToggleRecord")
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
