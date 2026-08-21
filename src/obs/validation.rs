use std::collections::HashSet;

use crate::domain::errors::ObsctlError;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};
use serde_json::Value;

/// Extract and validate an array of named OBS resources.
///
/// - Requires `list_key` to be present as an array.
/// - Requires every entry to be an object containing `name_key` as a JSON string.
/// - Trims and validates each token (rejecting blank/control-character values).
/// - Rejects duplicate names (after normalization).
pub fn extract_resource_names(
    value: &Value,
    list_key: &str,
    name_key: &str,
) -> Result<Vec<String>, ObsctlError> {
    collect_unique_names(
        value,
        list_key,
        &format!("'{name_key}' in '{list_key}'"),
        |item| {
            item.as_object()
                .and_then(|obj| obj.get(name_key))
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ObsctlError::ObsRequestFailed(format!(
                        "missing '{name_key}' in '{list_key}' item"
                    ))
                })
        },
    )
}

/// Extract and validate an array of plain string values (e.g. profile
/// names). Unlike `extract_resource_names`, entries are raw JSON strings
/// rather than `{ name_key: "..." }` objects.
pub fn extract_string_array(value: &Value, list_key: &str) -> Result<Vec<String>, ObsctlError> {
    collect_unique_names(value, list_key, &format!("entry in '{list_key}'"), |item| {
        item.as_str().ok_or_else(|| {
            ObsctlError::ObsRequestFailed(format!("non-string entry in '{list_key}'"))
        })
    })
}

/// The part both extractors have in common: require `list_key` to be an
/// array, pull one name out of each entry with `read_name`, validate it as a
/// target token, and reject any name that repeats.
///
/// `noun` names an entry in the error messages — `"'sceneName' in 'scenes'"`
/// or `"entry in 'profiles'"` — so the two callers keep their own wording
/// without keeping their own copy of the loop.
fn collect_unique_names<'a>(
    value: &'a Value,
    list_key: &str,
    noun: &str,
    read_name: impl Fn(&'a Value) -> Result<&'a str, ObsctlError>,
) -> Result<Vec<String>, ObsctlError> {
    let list = value
        .get(list_key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ObsctlError::ObsRequestFailed(format!("missing or invalid '{list_key}' payload"))
        })?;

    let mut names = Vec::with_capacity(list.len());
    let mut seen = HashSet::new();

    for item in list {
        let name = read_name(item)?;

        let trimmed = trim_and_validate_token_with_max_len(name, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| ObsctlError::ObsRequestFailed(format!("invalid {noun}: {error}")))?;

        if !seen.insert(trimmed.clone()) {
            return Err(ObsctlError::ObsRequestFailed(format!(
                "duplicate {noun}: '{trimmed}'"
            )));
        }

        names.push(trimmed);
    }

    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::extract_resource_names;
    use crate::support::validation::MAX_TARGET_TOKEN_LENGTH;
    use serde_json::json;

    #[test]
    fn extract_resource_names_rejects_control_characters() {
        let payload = json!({
            "inputs": [
                { "inputName": "Mic\t1" }
            ]
        });
        assert!(extract_resource_names(&payload, "inputs", "inputName").is_err());
    }

    #[test]
    fn extract_resource_names_rejects_excessive_length() {
        let name = "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1);
        let payload = json!({
            "inputs": [
                { "inputName": name }
            ]
        });
        assert!(extract_resource_names(&payload, "inputs", "inputName").is_err());
    }

    #[test]
    fn extract_resource_names_rejects_duplicates() {
        let payload = json!({
            "scenes": [
                { "sceneName": "Main" },
                { "sceneName": "Main" }
            ]
        });
        assert!(extract_resource_names(&payload, "scenes", "sceneName").is_err());
    }

    #[test]
    fn extract_resource_names_rejects_missing_name_field() {
        let payload = json!({
            "inputs": [ { "wrongField": "Mic" } ]
        });
        assert!(extract_resource_names(&payload, "inputs", "inputName").is_err());
    }

    #[test]
    fn extract_string_array_parses_plain_strings() {
        let payload = json!({ "profiles": ["Default", "Streaming"] });
        let names = super::extract_string_array(&payload, "profiles").unwrap();
        assert_eq!(names, vec!["Default".to_string(), "Streaming".to_string()]);
    }

    #[test]
    fn extract_string_array_rejects_non_string_entries() {
        let payload = json!({ "profiles": ["Default", 5] });
        assert!(super::extract_string_array(&payload, "profiles").is_err());
    }

    #[test]
    fn extract_string_array_rejects_duplicates() {
        let payload = json!({ "profiles": ["Default", "Default"] });
        assert!(super::extract_string_array(&payload, "profiles").is_err());
    }
}
