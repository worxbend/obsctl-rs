//! Turns raw obs-websocket event payloads into [`ObsEvent`] values.
//!
//! This is the validation half of the event path: everything that decides
//! whether a payload is well-formed enough to act on lives here, so that
//! `client.rs` only has to route a decoded event onto its channel.

use serde_json::Value;
use tracing::warn;

use crate::domain::volume::is_valid_multiplier;
use crate::obs::client::ObsEvent;
use crate::obs::validation::extract_resource_names;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// One OBS event payload, with the field lookups that every arm of the event
/// switch needs.
///
/// OBS sends `{"eventType": "...", "eventData": {...}}`. Every field read has
/// the same shape: pull the field, check its type (and, for strings, that it is
/// a safe token), and log exactly which field of which event was malformed
/// before giving up. Keeping that in one place is what lets the switch below
/// read as a list of event shapes instead of a wall of error handling.
pub(crate) struct EventPayload {
    event_type: String,
    data: Value,
}

impl EventPayload {
    /// Returns `None` — after logging — when the message has no usable
    /// `eventType`, which is the only field we cannot recover from.
    pub(crate) fn parse(message: Value) -> Option<Self> {
        let raw_type = message
            .get("eventType")
            .and_then(|v| v.as_str())
            .or_else(|| {
                warn!("Malformed OBS event: missing or invalid eventType");
                None
            })?;
        let event_type = trim_and_validate_token_with_max_len(raw_type, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| {
                warn!(
                    event_type = %raw_type,
                    message = %error,
                    "Malformed OBS event payload: invalid eventType"
                );
            })
            .ok()?;
        let data = message.get("eventData").cloned().unwrap_or(Value::Null);
        Some(Self { event_type, data })
    }

    fn warn_field(&self, field: &str, message: &str) {
        warn!(
            event_type = %self.event_type,
            field = %field,
            "{message}"
        );
    }

    fn required_str(&self, field: &str) -> Option<String> {
        let raw = self.data.get(field).and_then(|v| v.as_str()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid string field",
            );
            None
        })?;
        trim_and_validate_token_with_max_len(raw, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| {
                warn!(
                    event_type = %self.event_type,
                    field = %field,
                    message = %error,
                    "Malformed OBS event payload: string field invalid"
                );
            })
            .ok()
    }

    fn required_bool(&self, field: &str) -> Option<bool> {
        self.data.get(field).and_then(|v| v.as_bool()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid boolean field",
            );
            None
        })
    }

    fn required_f64(&self, field: &str) -> Option<f64> {
        self.data.get(field).and_then(|v| v.as_f64()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid number field",
            );
            None
        })
    }

    fn required_array(&self, field: &str) -> Option<&Vec<Value>> {
        self.data.get(field).and_then(|v| v.as_array()).or_else(|| {
            self.warn_field(
                field,
                "Malformed OBS event payload: missing or invalid array field",
            );
            None
        })
    }

    /// A number that is neither NaN nor an infinity.
    ///
    /// This is defence in depth rather than input validation — `serde_json`
    /// already rejects out-of-range numbers while parsing, so a NaN or
    /// infinity can only reach here from a payload built in-process.
    ///
    /// The value is otherwise unconstrained: a decibel level, for instance, is
    /// normally negative, since anything quieter than unity gain sits below
    /// 0 dB.
    fn required_finite_f64(&self, field: &str) -> Option<f64> {
        let value = self.required_f64(field)?;
        if !value.is_finite() {
            self.warn_field(field, "Malformed OBS event payload: level is not finite");
            return None;
        }
        Some(value)
    }

    /// A linear volume multiplier: finite and at or above zero, because a
    /// negative multiplier would invert the waveform rather than attenuate it.
    fn required_volume_multiplier(&self, field: &str) -> Option<f64> {
        let value = self.required_f64(field)?;
        if !is_valid_multiplier(value) {
            self.warn_field(
                field,
                "Malformed OBS event payload: volume multiplier must be finite and non-negative",
            );
            return None;
        }
        Some(value)
    }
}

/// Turns a validated payload into the internal event, or `None` when a field
/// the event cannot do without was malformed (already logged by the lookup).
pub(crate) fn translate_event(payload: EventPayload) -> Option<ObsEvent> {
    let event = match payload.event_type.as_str() {
        "CurrentProgramSceneChanged" => ObsEvent::CurrentProgramSceneChanged {
            scene_name: payload.required_str("sceneName")?,
        },
        "SceneCreated" | "SceneRemoved" | "SceneNameChanged" | "SceneListReindexed" => {
            ObsEvent::SceneListChanged
        }
        "InputCreated" => ObsEvent::InputCreated {
            input_name: payload.required_str("inputName")?,
        },
        "InputRemoved" => ObsEvent::InputRemoved {
            input_name: payload.required_str("inputName")?,
        },
        "InputMuteStateChanged" => ObsEvent::InputMuteStateChanged {
            input_name: payload.required_str("inputName")?,
            muted: payload.required_bool("inputMuted")?,
        },
        "InputVolumeChanged" => ObsEvent::InputVolumeChanged {
            input_name: payload.required_str("inputName")?,
            volume_mul: payload.required_volume_multiplier("inputVolumeMul")?,
            // Any finite number will do here: audio dB is normally negative.
            volume_db: payload.required_finite_f64("inputVolumeDb")?,
        },
        "InputVolumeMeters" => ObsEvent::InputVolumeMeters {
            inputs: parse_volume_meters(&payload)?,
        },
        "StreamStateChanged" => ObsEvent::StreamStateChanged {
            active: payload.required_bool("outputActive")?,
        },
        "RecordStateChanged" => ObsEvent::RecordStateChanged {
            active: payload.required_bool("outputActive")?,
        },
        "CurrentProfileChanged" => ObsEvent::CurrentProfileChanged {
            profile_name: payload.required_str("profileName")?,
        },
        "ProfileListChanged" => ObsEvent::ProfileListChanged,
        "CurrentSceneCollectionChanged" => ObsEvent::CurrentSceneCollectionChanged {
            scene_collection_name: payload.required_str("sceneCollectionName")?,
        },
        "SceneCollectionListChanged" => ObsEvent::SceneCollectionListChanged,
        _ => ObsEvent::Other {
            event_type: payload.event_type.clone(),
            data: payload.data.clone(),
        },
    };
    Some(event)
}

/// Reduces the `InputVolumeMeters` payload to one peak level per input.
///
/// OBS emits a `[magnitude, peak, inputPeak]` tuple per audio channel, so the
/// level shown for an input is the loudest magnitude across its channels. A
/// meter with no channels yet — an input that has not run its first audio
/// capture callback — is a valid silent reading, not a malformed entry.
///
/// Any genuinely invalid entry discards the whole event rather than reporting a
/// partial set of levels that would look like some inputs went silent.
fn parse_volume_meters(payload: &EventPayload) -> Option<Vec<(String, f32)>> {
    let inputs = payload.required_array("inputs")?;
    let input_names = extract_resource_names(&payload.data, "inputs", "inputName")
        .map_err(|error| {
            warn!(
                event_type = %payload.event_type,
                message = %error,
                "Malformed OBS InputVolumeMeters payload: invalid inputName list"
            );
        })
        .ok()?;

    let mut had_invalid_entry = false;
    let mut levels_by_input: Vec<(String, f32)> = Vec::new();

    for (name, input) in input_names.into_iter().zip(inputs.iter()) {
        let channels = match input.get("inputLevelsMul") {
            Some(Value::Array(channels)) => channels,
            None => {
                had_invalid_entry = true;
                continue;
            }
            Some(invalid) => {
                warn!(
                    event_type = %payload.event_type,
                    input = %name,
                    data = %invalid,
                    "Malformed OBS InputVolumeMeters payload: inputLevelsMul must be an array"
                );
                had_invalid_entry = true;
                continue;
            }
        };

        let mut peak = 0.0_f32;
        for channel in channels {
            match channel
                .as_array()
                .and_then(|values| values.first())
                .and_then(|value| value.as_f64())
                .map(|value| value as f32)
            {
                Some(magnitude) if magnitude.is_finite() && magnitude >= 0.0 => {
                    peak = peak.max(magnitude);
                }
                _ => had_invalid_entry = true,
            }
        }

        levels_by_input.push((name, peak));
    }

    if had_invalid_entry {
        warn!(
            event_type = %payload.event_type,
            "Malformed OBS InputVolumeMeters payload: discarding event due to invalid entries"
        );
        return None;
    }
    Some(levels_by_input)
}
