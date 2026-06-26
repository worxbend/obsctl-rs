use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "obsctl", version, about = "OBS Studio local controller")]
pub struct Cli {
    #[arg(long, global = true, help = "Config file path")]
    pub config: Option<PathBuf>,

    #[arg(
        long,
        global = true,
        value_name = "LEVEL",
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
        target: String,
    },
    Mute {
        target: String,
    },
    Unmute {
        target: String,
    },
    ToggleMute {
        target: String,
    },
    #[command(alias = "volume")]
    Vol {
        target: String,
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
