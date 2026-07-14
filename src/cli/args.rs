use crate::support::fs;
use crate::support::validation::{
    MAX_TARGET_TOKEN_LENGTH, trim_and_validate_path_token, trim_and_validate_token_with_max_len,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "obsctl", version, about = "OBS Studio local controller")]
pub struct Cli {
    #[arg(long, global = true, value_parser = non_blank_path, help = "Config file path")]
    pub config: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        value_name = "LEVEL",
        value_parser = parse_log_level,
        help = "Log level: trace|debug|info|warn|error"
    )]
    pub log_level: Option<String>,

    #[arg(
        short = 'v',
        long,
        global = true,
        help = "Enable debug logging (shorthand for --log-level debug)"
    )]
    pub verbose: bool,

    #[arg(
        long,
        global = true,
        help = "Force overwrite or override safety checks"
    )]
    pub force: bool,

    #[arg(
        long,
        global = true,
        help = "Output raw JSON instead of human-readable text"
    )]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Init,
    ValidateConfig,
    Server {
        #[arg(long)]
        headless: bool,
    },
    Tui,
    Status,
    ObsStatus,
    ServerStatus,
    Reconnect,
    ShutdownServer,
    Scene {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
    },
    Profile {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
    },
    #[command(alias = "scene-collection")]
    Collection {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
    },
    Mute {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
    },
    Unmute {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
    },
    ToggleMute {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
    },
    #[command(alias = "volume")]
    Vol {
        #[arg(value_parser = non_blank_trimmed)]
        target: String,
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        percent: u8,
    },
    DumpConfig,
    ReloadConfig,
    #[command(alias = "stream")]
    ToggleStream,
    #[command(alias = "record")]
    ToggleRecord,
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    Install,
    Uninstall,
    Status,
    Start,
    Stop,
    Restart,
}

fn non_blank_trimmed(value: &str) -> Result<String, String> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| format!("value {error}"))
}

pub(crate) fn parse_log_level(value: &str) -> Result<String, String> {
    let level = trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| format!("log level {error}"))?
        .to_ascii_lowercase();
    match level.as_str() {
        "trace" | "debug" | "info" | "warn" | "error" => Ok(level),
        _ => Err("log level must be one of trace, debug, info, warn, error".to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn non_blank_path_rejects_empty_or_whitespace() {
        assert!(non_blank_path("   ").is_err());
        assert!(non_blank_path("").is_err());
    }

    #[test]
    fn non_blank_path_rejects_relative_path() {
        assert!(non_blank_path("relative/config.yml").is_err());
    }

    #[test]
    fn non_blank_path_rejects_traversal() {
        assert!(non_blank_path("/tmp/../obsctl.yml").is_err());
    }

    #[test]
    fn non_blank_path_preserves_trimmed_path() {
        assert_eq!(
            non_blank_path("  /tmp/obsctl.yml  ").unwrap(),
            PathBuf::from("/tmp/obsctl.yml")
        );
    }

    #[test]
    fn valid_log_level_normalizes_and_rejects_bad_values() {
        assert_eq!(parse_log_level(" TrAcE ").unwrap(), "trace");
        assert!(parse_log_level("verbose").is_err());
    }

    #[test]
    fn non_blank_trimmed_rejects_control_characters() {
        assert!(non_blank_trimmed("main\tcam").is_err());
        assert!(non_blank_trimmed("main\ncam").is_err());
    }

    #[test]
    fn non_blank_trimmed_rejects_excessive_length() {
        let value = "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1);
        assert!(non_blank_trimmed(&value).is_err());
    }

    #[test]
    fn non_blank_path_rejects_control_characters() {
        assert!(non_blank_path("/tmp/\nobsctl.yml").is_err());
        assert!(non_blank_path("/tmp/\tobsctl.yml").is_err());
    }

    #[test]
    fn parse_log_level_rejects_excessive_length() {
        let value = "trace".repeat(100);
        assert!(parse_log_level(&value).is_err());
    }
}

fn non_blank_path(value: &str) -> Result<PathBuf, String> {
    let trimmed =
        trim_and_validate_path_token(value).map_err(|error| format!("config path {error}"))?;
    let path = PathBuf::from(&trimmed);
    if !path.is_absolute() {
        return Err("config path must be an absolute path".to_string());
    }
    if fs::has_path_traversal(std::path::Path::new(&trimmed)) {
        return Err("config path must not include traversal components".to_string());
    }
    Ok(path)
}
