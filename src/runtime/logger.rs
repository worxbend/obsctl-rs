use std::io;
use std::path::PathBuf;

use crate::support::fs;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn default_log_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.data_local_dir().join("obsctl/obsctl.log"))
}

/// Initialize tracing for server mode.
///
/// Writes to both stderr (human-readable) and optionally a log file.
/// `level` should be one of "debug", "info", "warn", "error".
pub fn init_server(level: &str, log_file: Option<PathBuf>) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(false);

    if let Some(path) = log_file {
        match open_safe_log_file(&path) {
            Ok(file) => {
                let file_layer = fmt::layer()
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .with_target(true);

                let _ = tracing_subscriber::registry()
                    .with(filter)
                    .with(stderr_layer)
                    .with(file_layer)
                    .try_init();
                return;
            }
            Err(error) => {
                tracing::warn!("failed to initialize log file {path:?}: {error}");
            }
        }
    }

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .try_init();
}

/// Minimal tracing init for CLI/TUI mode (stderr only).
/// `level` is an `EnvFilter`-compatible string such as "debug" or "warn".
pub fn init_cli(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(true))
        .try_init();
}

fn prepare_log_parent(parent: &std::path::Path) -> std::io::Result<()> {
    if parent.exists() {
        fs::ensure_private_dir(parent)
    } else {
        let probe = parent.join("obsctl.log");
        fs::ensure_private_parent(&probe)
    }
}

fn open_safe_log_file(path: &std::path::Path) -> io::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        prepare_log_parent(parent)?;
    }
    fs::ensure_path_not_symlink(path)?;

    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let file = {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW);
        options.open(path)?
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log path is not a regular file",
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_safe_log_file_creates_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("obsctl.log");

        let file = open_safe_log_file(&path).unwrap();
        drop(file);
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn open_safe_log_file_rejects_symlink_path() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.log");
        let link = dir.path().join("link.log");
        let _ = std::fs::File::create(&real).unwrap();
        symlink(&real, &link).unwrap();

        assert!(open_safe_log_file(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn open_safe_log_file_sets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("obsctl.log");

        let file = open_safe_log_file(&path).unwrap();
        let mode = file.metadata().unwrap().permissions().mode();
        drop(file);
        assert_eq!(mode & 0o777, 0o600);
        assert!(path.exists());
    }

    #[test]
    fn open_safe_log_file_rejects_directory_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested");
        std::fs::create_dir(&path).unwrap();

        assert!(open_safe_log_file(&path).is_err());
    }

    #[test]
    fn default_log_path_is_in_local_state() {
        if let Some(path) = default_log_path() {
            let s = path.to_string_lossy();
            assert!(s.contains("obsctl"), "path should contain obsctl: {s}");
            assert!(s.ends_with("obsctl.log"), "should end with obsctl.log: {s}");
        }
    }
}
