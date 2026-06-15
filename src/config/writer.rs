use std::io::Write as _;
use std::path::Path;

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

use super::model::Config;

pub fn write(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content =
        serde_yaml::to_string(config).map_err(|e| ObsctlError::ConfigInvalid(e.to_string()))?;
    let tmp = path.with_extension("yml.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.flush()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn write_default(path: &Path) -> Result<()> {
    write(&Config::default(), path)
}
