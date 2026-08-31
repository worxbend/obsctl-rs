use std::io::Write as _;
use std::path::Path;

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::support::fs;

use super::model::Config;

pub fn write_atomic(config: &Config, path: &Path) -> Result<()> {
    // `write_atomic_with_temp_file` re-checks the parent directory, but it
    // reports failures as `Io` (exit code 1); checking here first keeps an
    // unsafe config directory classified as `ConfigInvalid` (exit code 2).
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
    write_atomic(&Config::default(), path)
}
