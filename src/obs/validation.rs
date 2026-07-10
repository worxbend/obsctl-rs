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
    let list = value
        .get(list_key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ObsctlError::ObsRequestFailed(format!("missing or invalid '{list_key}' payload"))
        })?;

    let mut names = Vec::with_capacity(list.len());
    let mut dedup = HashSet::new();

    for item in list {
        let name = item
            .as_object()
            .and_then(|obj| obj.get(name_key))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ObsctlError::ObsRequestFailed(format!("missing '{name_key}' in '{list_key}' item"))
            })?;

        let trimmed = trim_and_validate_token_with_max_len(name, MAX_TARGET_TOKEN_LENGTH).map_err(
            |error| {
                ObsctlError::ObsRequestFailed(format!(
                    "invalid '{name_key}' in '{list_key}' item: {error}"
                ))
            },
        )?;

        if !dedup.insert(trimmed.clone()) {
            return Err(ObsctlError::ObsRequestFailed(format!(
                "duplicate '{name_key}' in '{list_key}': '{trimmed}'"
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
}
