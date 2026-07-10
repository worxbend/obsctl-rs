use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::domain::errors::ObsctlError;
use crate::obs::protocol::RequestData;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

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

type RequestResult<T> = Result<T, ObsctlError>;

fn validate_name(name: &str, field: &str) -> RequestResult<String> {
    trim_and_validate_token_with_max_len(name, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::ObsRequestFailed(format!("{field} must be valid: {error}")))
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

pub fn set_current_program_scene(scene_name: &str) -> RequestResult<RequestData> {
    let scene_name = validate_name(scene_name, "sceneName")?;
    Ok(req_with(
        "SetCurrentProgramScene",
        json!({ "sceneName": scene_name }),
    ))
}

pub fn get_input_list() -> RequestData {
    req("GetInputList")
}

pub fn get_input_mute(input_name: &str) -> RequestResult<RequestData> {
    let input_name = validate_name(input_name, "inputName")?;
    Ok(req_with("GetInputMute", json!({ "inputName": input_name })))
}

pub fn set_input_mute(input_name: &str, muted: bool) -> RequestResult<RequestData> {
    let input_name = validate_name(input_name, "inputName")?;
    Ok(req_with(
        "SetInputMute",
        json!({ "inputName": input_name, "inputMuted": muted }),
    ))
}

pub fn toggle_input_mute(input_name: &str) -> RequestResult<RequestData> {
    let input_name = validate_name(input_name, "inputName")?;
    Ok(req_with(
        "ToggleInputMute",
        json!({ "inputName": input_name }),
    ))
}

pub fn get_input_volume(input_name: &str) -> RequestResult<RequestData> {
    let input_name = validate_name(input_name, "inputName")?;
    Ok(req_with(
        "GetInputVolume",
        json!({ "inputName": input_name }),
    ))
}

pub fn set_input_volume(input_name: &str, volume_mul: f64) -> RequestResult<RequestData> {
    if !volume_mul.is_finite() || volume_mul < 0.0 {
        return Err(ObsctlError::ObsRequestFailed(
            "inputVolumeMul must be finite and non-negative".to_string(),
        ));
    }

    let input_name = validate_name(input_name, "inputName")?;
    Ok(req_with(
        "SetInputVolume",
        json!({ "inputName": input_name, "inputVolumeMul": volume_mul }),
    ))
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
        let r = set_current_program_scene("Main Scene").unwrap();
        let data = r.request_data.unwrap();
        assert_eq!(data["sceneName"], "Main Scene");
    }

    #[test]
    fn set_volume_includes_mul() {
        let r = set_input_volume("Mic", 0.5).unwrap();
        let data = r.request_data.unwrap();
        assert!((data["inputVolumeMul"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn set_volume_rejects_invalid_request_payloads() {
        assert!(set_input_volume("Mic\n", 0.5).is_err());
        assert!(set_input_volume("Mic", f64::NAN).is_err());
    }

    #[test]
    fn request_name_rejects_excessive_length() {
        let long_name = "a".repeat(crate::support::validation::MAX_TARGET_TOKEN_LENGTH + 1);
        assert!(set_current_program_scene(&long_name).is_err());
        assert!(set_input_volume(&long_name, 0.5).is_err());
        assert!(get_input_mute(&long_name).is_err());
        assert!(set_input_mute(&long_name, true).is_err());
        assert!(toggle_input_mute(&long_name).is_err());
        assert!(get_input_volume(&long_name).is_err());
    }

    #[test]
    fn request_name_is_trimmed_and_validated() {
        let r = set_current_program_scene("  Main Scene  ").unwrap();
        assert_eq!(r.request_data.as_ref().unwrap()["sceneName"], "Main Scene");

        assert!(set_current_program_scene("bad\nname").is_err());
    }
}
