use serde_json::Value;

pub fn redact_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for key in ["password", "authentication", "auth", "token"] {
                if map.contains_key(key) {
                    map.insert(key.to_string(), Value::String("[REDACTED]".to_string()));
                }
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
    fn plain_value_unchanged() {
        let mut v = json!("hello");
        redact_secrets(&mut v);
        assert_eq!(v, json!("hello"));
    }
}
