use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

pub fn encode(value: &impl serde::Serialize) -> Result<String> {
    let mut s =
        serde_json::to_string(value).map_err(|e| ObsctlError::IpcProtocolError(e.to_string()))?;
    s.push('\n');
    Ok(s)
}

pub fn decode<T: serde::de::DeserializeOwned>(line: &str) -> Result<T> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(ObsctlError::IpcProtocolError("empty IPC frame".to_string()));
    }
    serde_json::from_str(trimmed).map_err(|e| ObsctlError::IpcProtocolError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Sample {
        value: String,
    }

    #[test]
    fn round_trip() {
        let s = Sample {
            value: "hello".to_string(),
        };
        let encoded = encode(&s).unwrap();
        assert!(encoded.ends_with('\n'));
        let decoded: Sample = decode(&encoded).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn malformed_json_returns_error() {
        assert!(decode::<Sample>("not json").is_err());
    }

    #[test]
    fn empty_frame_is_error() {
        assert!(decode::<Sample>("").is_err());
        assert!(decode::<Sample>("\n").is_err());
        assert!(decode::<Sample>("\r\n").is_err());
        assert!(decode::<Sample>("   \n").is_err());
    }

    #[test]
    fn trim_newline_before_decode() {
        let value = decode::<Sample>("{\"value\":\"hello\"}\r\n").unwrap();
        assert_eq!(value.value, "hello");
    }
}
