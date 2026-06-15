use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

pub fn encode(value: &impl serde::Serialize) -> Result<String> {
    let mut s =
        serde_json::to_string(value).map_err(|e| ObsctlError::IpcProtocolError(e.to_string()))?;
    s.push('\n');
    Ok(s)
}

pub fn decode<T: serde::de::DeserializeOwned>(line: &str) -> Result<T> {
    serde_json::from_str(line.trim()).map_err(|e| ObsctlError::IpcProtocolError(e.to_string()))
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
}
