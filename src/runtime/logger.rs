// Tracing subscriber setup with optional file appender.
use std::path::PathBuf;

use tracing::Level;
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
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
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
    }

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .try_init();
}

/// Minimal tracing init for CLI/TUI mode (stderr only, info level).
pub fn init_cli() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(true))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_log_path_is_in_local_state() {
        if let Some(path) = default_log_path() {
            let s = path.to_string_lossy();
            assert!(s.contains("obsctl"), "path should contain obsctl: {s}");
            assert!(s.ends_with("obsctl.log"), "should end with obsctl.log: {s}");
        }
    }
}
