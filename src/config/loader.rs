use std::path::Path;

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::ipc::socket_path::resolve_server_socket_path;
use crate::support::fs;

use super::{model::Config, schema};

pub fn load(path: &Path) -> Result<Config> {
    load_with_warnings(path).map(|(config, _warnings)| config)
}

pub fn load_with_warnings(path: &Path) -> Result<(Config, Vec<schema::ValidationWarning>)> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(ObsctlError::ConfigNotFound(path.display().to_string()));
        }
        fs::ensure_private_dir(parent).map_err(|_| {
            ObsctlError::ConfigInvalid("config path parent directory is not private".to_string())
        })?;
    }

    fs::ensure_path_not_symlink(path).map_err(|_| {
        ObsctlError::ConfigInvalid("refusing to read symlinked config path".to_string())
    })?;

    let content = fs::read_file_no_follow(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ObsctlError::ConfigNotFound(path.display().to_string())
        } else {
            ObsctlError::Io(e)
        }
    })?;
    let mut config: Config =
        serde_yaml::from_str(&content).map_err(|e| ObsctlError::ConfigInvalid(e.to_string()))?;
    migrate_legacy_reconnect(&mut config);
    let warnings = schema::validate(&config)?;
    Ok((config, warnings))
}

/// Migrate the legacy `connection.reconnect` field to the canonical top-level `reconnect`.
fn migrate_legacy_reconnect(config: &mut Config) {
    if let Some(legacy) = config.connection.reconnect.take() {
        config.reconnect = legacy;
    }
}

pub fn load_or_default(path: &Path) -> Result<Config> {
    if path.exists() {
        load(path)
    } else {
        let config = Config::default();
        schema::validate(&config)?;
        Ok(config)
    }
}

/// Resolve the daemon socket path from the `server.socket_path` config field.
///
/// The `server.socket_path` prefix on the error message is the config field
/// name as the user wrote it, so a rejected path points at the line to fix.
/// That prefix is asserted by tests here and in `cli::router`; keep it.
pub(crate) fn resolve_configured_socket_path(
    configured: Option<&str>,
) -> Result<std::path::PathBuf> {
    resolve_server_socket_path(configured)
        .map_err(|error| ObsctlError::ConfigInvalid(format!("server.socket_path {error}")))
}

/// A config plus the daemon socket path it names.
#[derive(Debug)]
pub struct LoadedRuntime {
    pub config: Config,
    pub socket_path: std::path::PathBuf,
}

pub fn load_runtime(path: &Path) -> Result<LoadedRuntime> {
    let config = load_or_default(path)?;
    let socket_path = resolve_configured_socket_path(config.server.socket_path.as_deref())?;
    Ok(LoadedRuntime {
        config,
        socket_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn load_validates_schema() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\n\
             connection:\n\
             host: 127.0.0.1\n\
             port: 4455\n\
             password_env: \"\"\n\
             connect_timeout_ms: 0\n\
             request_timeout_ms: 2500\n",
        )
        .unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn rejects_port_out_of_u16_range() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\n\
             connection:\n\
             host: 127.0.0.1\n\
             port: 70000\n\
             password_env: \"\"\n\
             connect_timeout_ms: 3000\n\
             request_timeout_ms: 2500\n",
        )
        .unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn load_or_default_uses_default_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.yml");
        let config = load_or_default(&path).unwrap();
        assert_eq!(config.connection.host, "127.0.0.1");
        assert_eq!(config.connection.port, 4455);
    }

    #[test]
    fn load_runtime_uses_defaults_when_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("missing.yml");
        let rt = load_runtime(&path).unwrap();
        assert!(rt.socket_path.ends_with("obsctl.sock"));
        assert_eq!(
            rt.config.ui.refresh_interval_ms,
            Config::default().ui.refresh_interval_ms
        );
    }

    /// `server.pid_file` and `server.start_embedded_if_missing` used to be
    /// declared in the config model even though nothing ever read them, and
    /// they were documented in the README, so config files in the wild carry
    /// them. They have been removed from the model; this pins that removing
    /// them does not break those files. It works because only the top-level
    /// `Config` struct is marked `#[serde(deny_unknown_fields)]` — the nested
    /// `ServerConfig` is not, so serde's default behaviour of ignoring keys it
    /// does not recognise applies inside the `server:` section.
    #[test]
    fn load_ignores_removed_server_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\n\
             server:\n\
             \x20 pid_file: /run/obsctl.pid\n\
             \x20 start_embedded_if_missing: true\n\
             \x20 allow_remote_shutdown: true\n",
        )
        .unwrap();

        let config = load(&path).unwrap();
        assert!(config.server.allow_remote_shutdown);
    }

    #[test]
    fn resolve_configured_socket_path_prefixes_the_config_field_name() {
        let err = resolve_configured_socket_path(Some("relative.sock"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("server.socket_path"));
    }

    #[test]
    fn load_runtime_rejects_invalid_socket_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: 1\nserver:\n  socket_path: relative.sock\n").unwrap();
        let err = load_runtime(&path).unwrap_err().to_string();
        assert!(err.contains("server.socket_path"));
    }

    #[test]
    fn load_with_warnings_propagates_schema_warnings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\nconnection:\n  host: 127.0.0.1\n  port: 4455\n  password: hunter2\n  password_env: \"\"\n  connect_timeout_ms: 3000\n  request_timeout_ms: 2500\n",
        )
        .unwrap();

        let (_config, warnings) = load_with_warnings(&path).unwrap();
        assert!(
            !warnings.is_empty() && warnings.iter().any(|w| w.0.contains("plaintext password")),
            "expected plaintext password warning"
        );
    }

    #[test]
    fn load_rejects_zero_refresh_interval() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\n\
             connection:\n\
             host: 127.0.0.1\n\
             port: 4455\n\
             connect_timeout_ms: 3000\n\
             request_timeout_ms: 2500\n\
             ui:\n\
             refresh_interval_ms: 0\n",
        )
        .unwrap();
        assert!(load(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_config_path() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real-config.yml");
        let link = dir.path().join("config-link.yml");
        std::fs::write(&real, "version: 1\n").unwrap();
        symlink(&real, &link).unwrap();

        let err = load(&link).unwrap_err();
        assert!(err.to_string().contains("symlinked config path"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_config_with_unsafe_parent() {
        let dir = TempDir::new().unwrap();
        let unsafe_parent = dir.path().join("unsafe-parent");
        std::fs::create_dir(&unsafe_parent).unwrap();
        std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let config_path = unsafe_parent.join("config.yml");
        std::fs::write(
            &config_path,
            "version: 1\n\
             connection:\n\
             host: 127.0.0.1\n\
             port: 4455\n\
             password_env: \"\"\n\
             connect_timeout_ms: 3000\n\
             request_timeout_ms: 2500\n",
        )
        .unwrap();

        let err = load(&config_path).unwrap_err();
        assert!(err.to_string().contains("parent directory is not private"));
    }
}
