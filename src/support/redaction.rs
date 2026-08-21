use serde_json::Value;

pub const REDACTED_SECRET: &str = "[REDACTED]";

const SECRET_KEYS: [&str; 4] = ["authentication", "password", "token", "auth"];

pub fn redact_message(message: impl AsRef<str>) -> String {
    redact_associated_secret_values(message.as_ref())
}

pub fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let secret_keys: Vec<String> = map
                .keys()
                .filter(|key| is_secret_key(key))
                .cloned()
                .collect();
            for key in secret_keys {
                map.insert(key, Value::String(REDACTED_SECRET.to_string()));
            }
            for v in map.values_mut() {
                redact_json_value(v);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_json_value(v);
            }
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS
        .iter()
        .any(|secret_key| key.eq_ignore_ascii_case(secret_key))
}

fn redact_associated_secret_values(message: &str) -> String {
    let message = redact_url_credentials(message);
    let redacted = replace_found_ranges(&message, secret_value_range);
    redact_bearer_tokens(&redacted)
}

/// Walk `message` once, replacing every range `find` points at with the
/// redaction placeholder and copying everything else through unchanged.
///
/// The two redaction passes below differ only in what they look for, so the
/// walk — which has to respect UTF-8 boundaries and must never drop or
/// duplicate the text between matches — is written once. Getting that walk
/// subtly wrong in one copy and not the other is how a redactor ends up
/// emitting the secret it was meant to hide.
fn replace_found_ranges(
    message: &str,
    find: fn(&str, usize) -> Option<std::ops::Range<usize>>,
) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut scan_at = 0;
    let mut copy_from = 0;

    while scan_at < message.len() {
        if !message.is_char_boundary(scan_at) {
            scan_at += 1;
            continue;
        }

        let Some(value_range) = find(message, scan_at) else {
            scan_at += message[scan_at..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            continue;
        };

        redacted.push_str(&message[copy_from..value_range.start]);
        redacted.push_str(REDACTED_SECRET);
        scan_at = value_range.end;
        copy_from = value_range.end;
    }

    redacted.push_str(&message[copy_from..]);
    redacted
}

fn redact_url_credentials(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut cursor = 0;

    while let Some(scheme_offset) = message[cursor..].find("://") {
        let scheme_end = cursor + scheme_offset + 3;
        let host_end = url_authority_end(message, scheme_end);
        let Some(at_offset) = message[scheme_end..host_end].find('@') else {
            redacted.push_str(&message[cursor..host_end]);
            cursor = host_end;
            continue;
        };

        let at_index = scheme_end + at_offset;
        redacted.push_str(&message[cursor..scheme_end]);
        redacted.push_str(REDACTED_SECRET);
        redacted.push_str(&message[at_index..host_end]);
        cursor = host_end;
    }

    redacted.push_str(&message[cursor..]);
    redacted
}

fn url_authority_end(message: &str, authority_start: usize) -> usize {
    for (offset, ch) in message[authority_start..].char_indices() {
        if ch.is_whitespace() || matches!(ch, '/' | '?' | '#') {
            return authority_start + offset;
        }
    }
    message.len()
}

fn redact_bearer_tokens(message: &str) -> String {
    replace_found_ranges(message, bearer_token_value_range)
}

fn bearer_token_value_range(message: &str, bearer_start: usize) -> Option<std::ops::Range<usize>> {
    const BEARER: &str = "bearer";
    let bearer_end = bearer_start.checked_add(BEARER.len())?;
    if !keyword_matches_at(message, bearer_start, bearer_end, BEARER) {
        return None;
    }
    if !is_key_boundary_before(message, bearer_start) {
        return None;
    }

    let mut cursor = bearer_end;
    if !matches!(message[cursor..].chars().next(), Some(ch) if ch.is_whitespace()) {
        return None;
    }
    cursor = consume_whitespace(message, cursor);
    if cursor >= message.len() {
        return None;
    }

    unquoted_value_range(message, cursor)
}

fn secret_value_range(message: &str, key_start: usize) -> Option<std::ops::Range<usize>> {
    for key in SECRET_KEYS {
        let key_end = key_start.checked_add(key.len())?;
        if !keyword_matches_at(message, key_start, key_end, key) {
            continue;
        }
        if !is_key_boundary_before(message, key_start) {
            continue;
        }
        if let Some(range) = associated_value_range(message, key_end) {
            return Some(range);
        }
    }
    None
}

/// The span of an unquoted value starting at `cursor`, or `None` if there is
/// nothing there to redact.
///
/// An already-redacted placeholder is matched first so that redacting twice
/// leaves the text alone rather than redacting the placeholder itself — the
/// idempotence the rest of the crate relies on.
fn unquoted_value_range(message: &str, cursor: usize) -> Option<std::ops::Range<usize>> {
    let value_end = redacted_literal_end(message, cursor)
        .unwrap_or_else(|| unquoted_value_end(message, cursor));
    (value_end != cursor).then_some(cursor..value_end)
}

fn keyword_matches_at(message: &str, start: usize, end: usize, keyword: &str) -> bool {
    end <= message.len()
        && message.is_char_boundary(start)
        && message.is_char_boundary(end)
        && message[start..end].eq_ignore_ascii_case(keyword)
}

fn associated_value_range(message: &str, key_end: usize) -> Option<std::ops::Range<usize>> {
    if is_identifier_char(message[key_end..].chars().next()) {
        return None;
    }

    let mut cursor = key_end;
    cursor = consume_optional_key_quote(message, cursor);
    cursor = consume_whitespace(message, cursor);

    if !matches!(message[cursor..].chars().next(), Some(':') | Some('=')) {
        return None;
    }

    cursor += 1;
    cursor = consume_whitespace(message, cursor);

    if cursor >= message.len() {
        return None;
    }

    match message[cursor..].chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let value_start = cursor + quote.len_utf8();
            let value_end = quoted_value_end(message, value_start, quote);
            Some(value_start..value_end)
        }
        Some(_) => unquoted_value_range(message, cursor),
        None => None,
    }
}

fn consume_optional_key_quote(message: &str, cursor: usize) -> usize {
    match message[cursor..].chars().next() {
        Some('"' | '\'') => cursor + 1,
        _ => cursor,
    }
}

fn consume_whitespace(message: &str, mut cursor: usize) -> usize {
    while let Some(ch) = message[cursor..].chars().next() {
        if !ch.is_whitespace() {
            break;
        }
        cursor += ch.len_utf8();
    }
    cursor
}

fn quoted_value_end(message: &str, value_start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (offset, ch) in message[value_start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return value_start + offset;
        }
    }
    message.len()
}

fn unquoted_value_end(message: &str, value_start: usize) -> usize {
    for (offset, ch) in message[value_start..].char_indices() {
        if ch.is_whitespace() || matches!(ch, ',' | ';' | '}' | ']') {
            return value_start + offset;
        }
    }
    message.len()
}

fn redacted_literal_end(message: &str, value_start: usize) -> Option<usize> {
    message[value_start..]
        .starts_with(REDACTED_SECRET)
        .then_some(value_start + REDACTED_SECRET.len())
}

fn is_key_boundary_before(message: &str, key_start: usize) -> bool {
    if key_start == 0 {
        return true;
    }
    !is_identifier_char(message[..key_start].chars().next_back())
}

fn is_identifier_char(ch: Option<char>) -> bool {
    matches!(ch, Some(c) if c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_unicode_adjacent_secret_assignments() {
        let redacted = redact_message("ошибка: password=секрет token=\"значение\"");

        assert_eq!(redacted, "ошибка: password=[REDACTED] token=\"[REDACTED]\"");
        assert!(!redacted.contains("секрет"));
        assert!(!redacted.contains("значение"));
    }

    #[test]
    fn redacts_url_encoded_credentials() {
        let redacted =
            redact_message("connect failed for wss://studio%40room:hunter%202@example.test/obs");

        assert_eq!(
            redacted,
            "connect failed for wss://[REDACTED]@example.test/obs"
        );
        assert!(!redacted.contains("studio%40room"));
        assert!(!redacted.contains("hunter%202"));
    }

    #[test]
    fn repeated_message_redaction_is_idempotent() {
        let once = redact_message(
            "Password='hunter2' token=abc.def Authorization: Bearer eyJ.secret http://user:pw@example.test",
        );
        let twice = redact_message(&once);

        assert_eq!(twice, once);
        assert_eq!(once.matches(REDACTED_SECRET).count(), 4);
    }

    #[test]
    fn redacts_mixed_case_sensitive_keys() {
        let redacted = redact_message("Password='hunter2' AUTH: abc123");

        assert_eq!(redacted, "Password='[REDACTED]' AUTH: [REDACTED]");
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains("abc123"));
    }

    #[test]
    fn redacts_bearer_tokens() {
        let redacted = redact_message("upstream rejected Authorization: Bearer eyJ.secret.token");

        assert_eq!(
            redacted,
            "upstream rejected Authorization: Bearer [REDACTED]"
        );
        assert!(!redacted.contains("eyJ.secret.token"));
    }

    #[test]
    fn redacts_structured_json_secret_fields() {
        let mut value = json!({
            "Password": "hunter2",
            "safe": "ok",
            "nested": {
                "authentication": "abc123",
                "items": [{ "AUTH": "bearer" }, { "token": "[REDACTED]" }]
            }
        });

        redact_json_value(&mut value);
        let once = value.clone();
        redact_json_value(&mut value);

        assert_eq!(value, once);
        assert_eq!(value["Password"], REDACTED_SECRET);
        assert_eq!(value["nested"]["authentication"], REDACTED_SECRET);
        assert_eq!(value["nested"]["items"][0]["AUTH"], REDACTED_SECRET);
        assert_eq!(value["nested"]["items"][1]["token"], REDACTED_SECRET);
        assert_eq!(value["safe"], "ok");
    }

    #[test]
    fn does_not_mask_unassociated_words() {
        let redacted = redact_message("password_env=OBS_WEBSOCKET_PASSWORD auth mode disabled");

        assert_eq!(
            redacted,
            "password_env=OBS_WEBSOCKET_PASSWORD auth mode disabled"
        );
    }
}
