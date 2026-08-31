use std::io;
use std::path::PathBuf;

use crate::support::fs;
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// The five log verbosities `obsctl` accepts on `--log-level` and `RUST_LOG`.
///
/// Parsing once into this enum means the rest of the program carries a proof
/// that the level is valid, instead of passing a `String` around and re-parsing
/// it (with a silent fallback) at every use site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFilterLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogFilterLevel {
    /// The canonical lowercase spelling, which is also a valid `EnvFilter`
    /// directive.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::str::FromStr for LogFilterLevel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let level = trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
            .map_err(|error| format!("log level {error}"))?
            .to_ascii_lowercase();
        match level.as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err("log level must be one of trace, debug, info, warn, error".to_string()),
        }
    }
}

pub fn default_log_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.data_local_dir().join("obsctl/obsctl.log"))
}

/// Initialize tracing for server mode.
///
/// Writes to both stderr (human-readable) and optionally a log file.
pub fn init_server(level: LogFilterLevel, log_file: Option<PathBuf>) {
    let filter = EnvFilter::new(level.as_str());

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(true)
        .with_target(false);

    // Open the log file first but hold any error until after `try_init`: a
    // `warn!` emitted before a subscriber exists is silently dropped.
    let mut open_error = None;
    let file = log_file.and_then(|path| match open_safe_log_file(&path) {
        Ok(file) => Some(file),
        Err(error) => {
            open_error = Some((path, error));
            None
        }
    });

    if let Some(file) = file {
        let file_layer = fmt::layer()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_target(true);

        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .with(file_layer)
            .try_init();
    } else {
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .try_init();
    }

    if let Some((path, error)) = open_error {
        tracing::warn!("failed to initialize log file {path:?}: {error}");
    }
}

/// Minimal tracing init for CLI/TUI mode (stderr only).
pub fn init_cli(level: LogFilterLevel) {
    let filter = EnvFilter::new(level.as_str());
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
    fn log_filter_level_parses_the_five_accepted_spellings() {
        use std::str::FromStr;

        assert_eq!(
            LogFilterLevel::from_str(" TrAcE ").unwrap(),
            LogFilterLevel::Trace
        );
        for level in [
            LogFilterLevel::Trace,
            LogFilterLevel::Debug,
            LogFilterLevel::Info,
            LogFilterLevel::Warn,
            LogFilterLevel::Error,
        ] {
            assert_eq!(LogFilterLevel::from_str(level.as_str()).unwrap(), level);
            assert!(EnvFilter::try_new(level.as_str()).is_ok());
        }
        assert!(LogFilterLevel::from_str("verbose").is_err());
        assert!(LogFilterLevel::from_str(&"trace".repeat(100)).is_err());
    }

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
