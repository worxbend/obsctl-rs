//! Payload parsing and validation for IPC commands.
//!
//! Split out of [`super::command_executor`] so the executor file holds the
//! command handlers themselves: nothing here touches OBS, the state store or
//! the config — it only turns a wire payload into checked values, or into a
//! `CommandParseError`.

use std::collections::HashSet;

use serde_json::Value;

use crate::domain::{errors::ObsctlError, result::Result};
use crate::ipc::protocol::ServerCommand;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// Reject a payload whose shape does not match what the command declares in
/// `ipc::protocol::COMMANDS`, before any OBS request is attempted.
pub(super) fn validate_payload(command: ServerCommand, args: &Value) -> Result<()> {
    match (command.required_args(), command.optional_args()) {
        ([], []) => validate_empty_payload(args, command.name()),
        (required, optional) => validate_object_args(args, command.name(), required, optional),
    }
}

#[cfg(test)]
pub(super) fn validate_command_payload(command: &str, args: &Value) -> Result<()> {
    validate_payload(parse_server_command(command)?, args)
}

pub(super) fn parse_server_command(name: &str) -> Result<ServerCommand> {
    ServerCommand::parse(name)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("unknown command: {name}")))
}

pub(super) fn required_string(args: &Value, key: &str) -> Result<String> {
    let raw = args
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))?;

    trim_and_validate_token_with_max_len(&raw, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::CommandParseError(format!("{key} {error}")))
}

/// An argument the payload is allowed to leave out.
///
/// Absent and explicitly `null` both mean "not given". Anything else goes
/// through the same validation as a required argument: a client that sent the
/// key meant something by it, and quietly ignoring an unusable value would
/// turn a rename into a second profile.
pub(super) fn optional_string(args: &Value, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => required_string(args, key).map(Some),
    }
}

/// Most scenes one scene profile may hide.
///
/// 128 names of at most `MAX_TARGET_TOKEN_LENGTH` (256) bytes each, plus JSON
/// quoting and commas, stays well inside the 64 KiB
/// [`MAX_IPC_LINE_BYTES`](crate::ipc::protocol::MAX_IPC_LINE_BYTES) frame the
/// request has to fit in — and so does the snapshot that later carries the
/// saved list back out to every subscriber.
pub(super) const MAX_HIDDEN_SCENES_PER_PROFILE: usize = 128;

/// The list of scene names under `key`, checked the same way a single `target`
/// is.
///
/// Every failure is a `CommandParseError`: the payload is malformed, which is
/// the client's mistake and not a statement about OBS or the config. Names
/// that differ only in case are the same scene everywhere else in obsctl, so
/// repeats collapse here too — the first spelling is the one kept, because
/// that is the one the caller listed.
pub(super) fn required_string_array(args: &Value, key: &str) -> Result<Vec<String>> {
    let items = args.get(key).and_then(Value::as_array).ok_or_else(|| {
        ObsctlError::CommandParseError(format!("{key} must be an array of scene names"))
    })?;

    if items.len() > MAX_HIDDEN_SCENES_PER_PROFILE {
        return Err(ObsctlError::CommandParseError(format!(
            "{key} may name at most {MAX_HIDDEN_SCENES_PER_PROFILE} scenes"
        )));
    }

    let mut seen = HashSet::new();
    let mut names = Vec::with_capacity(items.len());
    for item in items {
        let raw = item.as_str().ok_or_else(|| {
            ObsctlError::CommandParseError(format!("{key} must be an array of scene names"))
        })?;
        let name = trim_and_validate_token_with_max_len(raw, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| ObsctlError::CommandParseError(format!("{key} entry {error}")))?;

        if seen.insert(name.to_ascii_lowercase()) {
            names.push(name);
        }
    }

    Ok(names)
}

pub(super) fn required_u8_percentage(args: &Value, key: &str) -> Result<u8> {
    let value = args
        .get(key)
        .ok_or_else(|| ObsctlError::CommandParseError(format!("missing {key}")))?;

    // `as_u64` rejects negatives and non-integers (including 50.5) for us, so
    // the only remaining question is the 0-100 range.
    match value.as_u64() {
        Some(percent) if percent <= 100 => Ok(percent as u8),
        _ => Err(ObsctlError::CommandParseError(format!(
            "{key} must be an integer 0-100"
        ))),
    }
}

pub(super) fn validate_object_args(
    args: &Value,
    command: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<()> {
    let object = args.as_object().ok_or_else(|| {
        ObsctlError::CommandParseError(format!("command {command} requires an object payload"))
    })?;

    for (key, _) in object {
        if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
            return Err(ObsctlError::CommandParseError(format!(
                "command {command} received unexpected argument '{key}'"
            )));
        }
    }

    for key in required {
        if !object.contains_key(*key) {
            return Err(ObsctlError::CommandParseError(format!(
                "command {command} missing required argument '{key}'"
            )));
        }
    }

    Ok(())
}

pub(super) fn validate_empty_payload(args: &Value, command: &str) -> Result<()> {
    if args.is_null() {
        return Ok(());
    }

    if let Some(object) = args.as_object()
        && object.is_empty()
    {
        return Ok(());
    }

    Err(ObsctlError::CommandParseError(format!(
        "command {command} does not accept arguments"
    )))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_HIDDEN_SCENES_PER_PROFILE, MAX_TARGET_TOKEN_LENGTH, parse_server_command,
        required_string, required_string_array, required_u8_percentage,
    };
    use super::{validate_command_payload, validate_empty_payload, validate_object_args};
    use serde_json::json;

    #[test]
    fn required_string_rejects_control_characters_and_empty_values() {
        let args = json!({ "target": "\t" });
        assert!(required_string(&args, "target").is_err());

        let args = json!({ "target": "" });
        assert!(required_string(&args, "target").is_err());

        let args = json!({ "target": 42 });
        assert!(required_string(&args, "target").is_err());

        let args = json!({ "target": " Main Scene " });
        assert_eq!(
            required_string(&args, "target").unwrap(),
            "Main Scene".to_string()
        );

        let args = json!({ "target": "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1) });
        assert!(required_string(&args, "target").is_err());
    }

    #[test]
    fn required_u8_percentage_requires_integer_in_range() {
        let args = json!({ "percent": 42 });
        assert_eq!(required_u8_percentage(&args, "percent").unwrap(), 42);

        let args = json!({ "percent": 150 });
        assert!(required_u8_percentage(&args, "percent").is_err());

        let args = json!({ "percent": -1 });
        assert!(required_u8_percentage(&args, "percent").is_err());

        let args = json!({ "percent": 50.5 });
        assert!(required_u8_percentage(&args, "percent").is_err());

        let args = json!({});
        assert!(required_u8_percentage(&args, "percent").is_err());
    }

    #[test]
    fn required_string_array_requires_an_array_of_usable_names() {
        let args = json!({ "hidden": ["Utility BG", " Overlay Src "] });
        assert_eq!(
            required_string_array(&args, "hidden").unwrap(),
            vec!["Utility BG".to_string(), "Overlay Src".to_string()],
            "entries are trimmed, exactly as a single target is"
        );

        let args = json!({ "hidden": [] });
        assert!(
            required_string_array(&args, "hidden").unwrap().is_empty(),
            "hiding nothing is a legal thing to save"
        );

        // Not an array at all, and an array carrying something that is not a
        // scene name.
        assert!(required_string_array(&json!({ "hidden": "Main" }), "hidden").is_err());
        assert!(required_string_array(&json!({}), "hidden").is_err());
        assert!(required_string_array(&json!({ "hidden": [42] }), "hidden").is_err());
        assert!(required_string_array(&json!({ "hidden": ["  "] }), "hidden").is_err());
        assert!(required_string_array(&json!({ "hidden": ["a\tb"] }), "hidden").is_err());

        let too_long = "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1);
        assert!(required_string_array(&json!({ "hidden": [too_long] }), "hidden").is_err());
    }

    #[test]
    fn required_string_array_caps_the_list_length() {
        let at_limit: Vec<String> = (0..MAX_HIDDEN_SCENES_PER_PROFILE)
            .map(|index| format!("Scene {index}"))
            .collect();
        assert_eq!(
            required_string_array(&json!({ "hidden": at_limit }), "hidden")
                .unwrap()
                .len(),
            MAX_HIDDEN_SCENES_PER_PROFILE
        );

        let over_limit: Vec<String> = (0..=MAX_HIDDEN_SCENES_PER_PROFILE)
            .map(|index| format!("Scene {index}"))
            .collect();
        assert!(required_string_array(&json!({ "hidden": over_limit }), "hidden").is_err());
    }

    #[test]
    fn required_string_array_drops_repeats_keeping_the_first_spelling() {
        let args = json!({ "hidden": ["Utility BG", "utility bg", " UTILITY BG "] });

        assert_eq!(
            required_string_array(&args, "hidden").unwrap(),
            vec!["Utility BG".to_string()]
        );
    }

    #[test]
    fn save_scene_profile_payloads_are_checked_against_the_declared_shape() {
        assert!(
            validate_command_payload("save_scene_profile", &json!({ "target": "streaming" }))
                .is_err(),
            "hidden is declared, so it is required"
        );
        assert!(
            validate_command_payload(
                "save_scene_profile",
                &json!({ "target": "streaming", "hidden": [], "extra": true }),
            )
            .is_err()
        );
        assert!(
            validate_command_payload(
                "save_scene_profile",
                &json!({ "target": "streaming", "hidden": ["Utility BG"] }),
            )
            .is_ok(),
            "an array value is legal: the payload check looks at key names"
        );
        assert!(validate_command_payload("clear_scene_profile", &json!(null)).is_ok());
        assert!(validate_command_payload("list_scene_profiles", &json!({})).is_ok());
    }

    #[test]
    fn validate_object_args_rejects_extra_payload_fields() {
        let args = json!({
            "target": "Mic",
            "extra": "boom",
        });
        assert!(validate_object_args(&args, "mute", &["target"], &[]).is_err());
    }

    #[test]
    fn validate_object_args_rejects_missing_payload_fields() {
        let args = json!({
            "target": "Mic",
        });
        assert!(validate_object_args(&args, "set_volume", &["target", "percent"], &[]).is_err());
    }

    #[test]
    fn validate_object_args_rejects_non_object_payload() {
        assert!(validate_object_args(&json!(null), "set_scene", &["target"], &[]).is_err());
        assert!(validate_object_args(&json!("string"), "set_scene", &["target"], &[]).is_err());
    }

    #[test]
    fn validate_empty_payload_rejects_argument_objects() {
        assert!(validate_empty_payload(&json!({ "extra": true }), "ping").is_err());
    }

    #[test]
    fn validate_empty_payload_rejects_non_empty_non_object_payload() {
        assert!(validate_empty_payload(&json!([]), "ping").is_err());
        assert!(validate_empty_payload(&json!("x"), "ping").is_err());
    }

    #[test]
    fn validate_empty_payload_allows_empty_object_or_null() {
        assert!(validate_empty_payload(&json!(null), "ping").is_ok());
        assert!(validate_empty_payload(&json!({}), "ping").is_ok());
    }

    #[test]
    fn validate_command_payload_rejects_unknown_command() {
        assert!(validate_command_payload("does-not-exist", &json!(null)).is_err());
    }

    #[test]
    fn validate_command_payload_rejects_wrong_shape_per_command() {
        assert!(validate_command_payload("set_volume", &json!({ "target": "Mic" })).is_err());
        assert!(validate_command_payload("toggle_stream", &json!({ "extra": true }),).is_err());
    }

    /// Every command name and its argument shape live in
    /// `ipc::protocol::COMMANDS`, which owns the name round-trip test. The
    /// executor's own job is to turn a name that is not in that table into a
    /// parse error rather than a panic.
    #[test]
    fn parse_server_command_rejects_unknown_name() {
        assert!(parse_server_command("does-not-exist").is_err());
    }
}
