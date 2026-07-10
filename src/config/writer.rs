use std::io::Write as _;
use std::path::Path;

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::support::fs;

use super::model::Config;

pub fn write(config: &Config, path: &Path) -> Result<()> {
    write_atomic(config, path)
}

pub fn write_atomic(config: &Config, path: &Path) -> Result<()> {
    fs::ensure_private_parent(path).map_err(|e| ObsctlError::ConfigInvalid(e.to_string()))?;

    let content =
        serde_yaml::to_string(config).map_err(|e| ObsctlError::ConfigInvalid(e.to_string()))?;

    fs::write_atomic_with_temp_file(path, "obsctl-config", 0o600, true, |tmp| {
        tmp.write_all(content.as_bytes())
    })
    .map_err(ObsctlError::Io)?;
    Ok(())
}

pub fn write_default(path: &Path) -> Result<()> {
    write(&Config::default(), path)
}
