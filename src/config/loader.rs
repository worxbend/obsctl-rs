use std::path::Path;

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

use super::model::Config;

pub fn load(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ObsctlError::ConfigNotFound(path.display().to_string())
        } else {
            ObsctlError::Io(e)
        }
    })?;
    serde_yaml::from_str(&content).map_err(|e| ObsctlError::ConfigInvalid(e.to_string()))
}

pub fn load_or_default(path: &Path) -> Result<Config> {
    if path.exists() {
        load(path)
    } else {
        Ok(Config::default())
    }
}
