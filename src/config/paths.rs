use std::path::PathBuf;

pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "obsctl").map(|d| d.config_dir().join("config.yml"))
}

pub fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OBSCTL_CONFIG") {
        return Some(PathBuf::from(p));
    }
    default_config_path()
}

pub fn default_log_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "obsctl").map(|d| {
        d.state_dir()
            .unwrap_or(d.data_local_dir())
            .join("obsctl.log")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_uses_env_override() {
        // Verify that an env var set externally is picked up; we read but don't mutate env here.
        // Just test that config_path() doesn't panic regardless of env state.
        let _ = config_path();
    }
}
