use serde_json::Value;

pub fn redact_secrets(value: &mut Value) {
    crate::support::redaction::redact_json_value(value);
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
