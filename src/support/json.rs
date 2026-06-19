use serde_json::Value;

pub fn redact_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let secret_keys: Vec<String> = map
                .keys()
                .filter(|key| is_secret_key(key))
                .cloned()
                .collect();
            for key in secret_keys {
                map.insert(key, Value::String("[REDACTED]".to_string()));
            }
            for v in map.values_mut() {
                redact_secrets(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_secrets(v);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "password" | "authentication" | "auth" | "token"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_password_field() {
        let mut v = json!({"password": "hunter2", "other": "ok"});
        redact_secrets(&mut v);
        assert_eq!(v["password"], "[REDACTED]");
        assert_eq!(v["other"], "ok");
    }

    #[test]
    fn redacts_nested_authentication() {
        let mut v = json!({"data": {"authentication": "abc123"}});
        redact_secrets(&mut v);
        assert_eq!(v["data"]["authentication"], "[REDACTED]");
    }

    #[test]
    fn redacts_mixed_case_secret_fields() {
        let mut v = json!({"Password": "hunter2", "AUTH": "abc123"});
        redact_secrets(&mut v);
        assert_eq!(v["Password"], "[REDACTED]");
        assert_eq!(v["AUTH"], "[REDACTED]");
    }

    #[test]
    fn plain_value_unchanged() {
        let mut v = json!("hello");
        redact_secrets(&mut v);
        assert_eq!(v, json!("hello"));
    }
}
