// CLI command router.

use std::path::PathBuf;

use crate::{
    cli::{
        args::{Cli, Commands, ServiceAction},
        client_commands::ProxyCtx,
    },
    config::{paths, schema, writer},
    ipc::socket_path::default_socket_path,
    runtime::logger,
    server::{daemon, options::ServerOptions},
    service::systemd_user_service::{self, CommandRunner, SystemctlRunner},
};

pub fn run(cli: Cli) -> i32 {
    let config_path = cli
        .config
        .clone()
        .or_else(|| std::env::var("OBSCTL_CONFIG").ok().map(PathBuf::from))
        .or_else(paths::config_path);

    let level = effective_log_level(&cli);

    match cli.command {
        None | Some(Commands::Tui) => {
            logger::init_cli(&level);
            run_tui(config_path)
        }
        Some(Commands::Init) => {
            logger::init_cli(&level);
            run_init(config_path, cli.force)
        }
        Some(Commands::ValidateConfig) => {
            logger::init_cli(&level);
            run_validate_config(config_path)
        }
        Some(Commands::Server { headless }) => {
            logger::init_server(&level, logger::default_log_path());
            run_server(config_path, headless)
        }
        Some(Commands::Service { action }) => {
            logger::init_cli(&level);
            run_service(action)
        }
        cmd => {
            logger::init_cli(&level);
            run_proxy(config_path, cmd.unwrap(), cli.json)
        }
    }
}

/// Derive the effective log level from CLI flags.
///
/// Priority: `--verbose` > `--log-level` > `RUST_LOG` env var > mode default.
fn effective_log_level(cli: &Cli) -> String {
    if cli.verbose {
        return "debug".to_string();
    }
    if let Some(ref level) = cli.log_level {
        return level.clone();
    }
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        return rust_log;
    }
    match &cli.command {
        Some(Commands::Server { .. }) => "info",
        _ => "warn",
    }
    .to_string()
}

fn resolve_socket_path(config_path: Option<&PathBuf>) -> PathBuf {
    if let Some(cp) = config_path
        && let Ok(config) = crate::config::loader::load_or_default(cp)
        && let Some(sp) = config.server.socket_path
    {
        return PathBuf::from(sp);
    }
    default_socket_path()
}

// ── Local commands ────────────────────────────────────────────────────────────

fn run_init(config_path: Option<PathBuf>, force: bool) -> i32 {
    let path = match config_path.or_else(paths::default_config_path) {
        Some(p) => p,
        None => {
            eprintln!("error: could not determine config directory");
            return 1;
        }
    };

    if path.exists() && !force {
        eprintln!(
            "Config already exists at {}. Use --force to overwrite.",
            path.display()
        );
        return 1;
    }

    match writer::write_default(&path) {
        Ok(()) => {
            println!("Initialized config at {}", path.display());
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn run_validate_config(config_path: Option<PathBuf>) -> i32 {
    let path = match config_path.or_else(paths::config_path) {
        Some(p) => p,
        None => {
            eprintln!("error: could not determine config path");
            return 2;
        }
    };

    let config = match crate::config::loader::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    match schema::validate(&config) {
        Ok(warnings) => {
            for w in &warnings {
                eprintln!("warning: {}", w.0);
            }
            if warnings.is_empty() {
                println!("Config is valid.");
            } else {
                println!("Config is valid with {} warning(s).", warnings.len());
            }
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

fn run_server(config_path: Option<PathBuf>, headless: bool) -> i32 {
    // Resolve the config path the daemon would use, so setup writes to the right place.
    let effective_path = config_path
        .clone()
        .or_else(paths::config_path)
        .or_else(paths::default_config_path);

    if let Some(ref path) = effective_path
        && !path.exists()
        && !headless
        && let Err(e) = first_time_setup(path)
    {
        eprintln!("Setup failed: {e}");
        return 1;
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let options = ServerOptions {
        headless,
        config_path,
    };

    rt.block_on(daemon::run(options))
}

/// Interactively prompt for OBS connection details and write a minimal config file.
fn first_time_setup(config_path: &std::path::Path) -> std::io::Result<()> {
    eprintln!("No config found at {}.", config_path.display());
    eprintln!("Let's create a minimal config.\n");

    let stdin = std::io::stdin();
    let mut buf = String::new();

    let host = prompt_line(&stdin, &mut buf, "OBS host", "127.0.0.1")?;
    let port_str = prompt_line(&stdin, &mut buf, "OBS port", "4455")?;
    let port: u16 = port_str.parse().unwrap_or_else(|_| {
        eprintln!("  Invalid port, using 4455.");
        4455
    });
    let password = prompt_line(&stdin, &mut buf, "OBS WebSocket password (blank = no auth)", "")?;

    let mut config = crate::config::model::Config::default();
    config.connection.host = host;
    config.connection.port = port;
    config.connection.password_env = String::new();
    if !password.is_empty() {
        config.connection.password = Some(password);
    }

    crate::config::writer::write(&config, config_path)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    eprintln!("\nConfig written to {}.\n", config_path.display());
    Ok(())
}

fn prompt_line(
    stdin: &std::io::Stdin,
    buf: &mut String,
    label: &str,
    default: &str,
) -> std::io::Result<String> {
    use std::io::{BufRead as _, Write as _};

    if default.is_empty() {
        eprint!("  {label}: ");
    } else {
        eprint!("  {label} [{default}]: ");
    }
    std::io::stderr().flush()?;

    buf.clear();
    stdin.lock().read_line(buf)?;
    let trimmed = buf.trim().to_string();
    Ok(if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed
    })
}

fn run_tui(config_path: Option<PathBuf>) -> i32 {
    let socket_path = resolve_socket_path(config_path.as_ref());
    let refresh_ms = config_path
        .as_ref()
        .and_then(|p| crate::config::loader::load_or_default(p).ok())
        .map(|c| c.ui.refresh_interval_ms)
        .unwrap_or(250);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    match rt.block_on(crate::tui::app::run(&socket_path, refresh_ms)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tui error: {e}");
            1
        }
    }
}

// ── Service commands ──────────────────────────────────────────────────────────

fn run_service(action: ServiceAction) -> i32 {
    let runner = SystemctlRunner;
    let unit_path = match systemd_user_service::unit_file_path() {
        Some(p) => p,
        None => {
            eprintln!("error: could not determine systemd user unit directory");
            return 1;
        }
    };

    match action {
        ServiceAction::Install => {
            let exec = match std::env::current_exe() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("error: could not determine executable path: {e}");
                    return 1;
                }
            };
            let content = systemd_user_service::unit_file_content(&exec);
            if let Some(parent) = unit_path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!("error: {e}");
                return 1;
            }
            if let Err(e) = std::fs::write(&unit_path, &content) {
                eprintln!("error writing unit file: {e}");
                return 1;
            }
            match runner.run("systemctl", &["--user", "daemon-reload"]) {
                Ok(_) => {
                    println!("Service installed at {}", unit_path.display());
                    println!("Enable with: systemctl --user enable --now obsctl.service");
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        ServiceAction::Uninstall => {
            if unit_path.exists()
                && let Err(e) = std::fs::remove_file(&unit_path)
            {
                eprintln!("error removing unit file: {e}");
                return 1;
            }
            match runner.run("systemctl", &["--user", "daemon-reload"]) {
                Ok(_) => {
                    println!("Service uninstalled.");
                    0
                }
                Err(e) => {
                    eprintln!("warning: daemon-reload failed: {e}");
                    0
                }
            }
        }
        ServiceAction::Status => {
            match runner.run("systemctl", &["--user", "status", "obsctl.service"]) {
                Ok(out) => {
                    print!("{out}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        ServiceAction::Start => service_ctl(&runner, "start"),
        ServiceAction::Stop => service_ctl(&runner, "stop"),
        ServiceAction::Restart => service_ctl(&runner, "restart"),
    }
}

fn service_ctl(runner: &dyn CommandRunner, verb: &str) -> i32 {
    match runner.run("systemctl", &["--user", verb, "obsctl.service"]) {
        Ok(_) => {
            println!("Service {verb}ed.");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

// ── Proxy commands ────────────────────────────────────────────────────────────

fn run_proxy(config_path: Option<PathBuf>, cmd: Commands, json_output: bool) -> i32 {
    let socket_path = resolve_socket_path(config_path.as_ref());
    let ctx = ProxyCtx {
        socket_path,
        json_output,
    };

    match cmd {
        Commands::Status => ctx.status(),
        Commands::ObsStatus => ctx.obs_status(),
        Commands::ServerStatus => ctx.server_status(),
        Commands::Reconnect => ctx.reconnect(),
        Commands::ShutdownServer => ctx.shutdown_server(),
        Commands::Scene { target } => ctx.scene(&target),
        Commands::Mute { target } => ctx.mute(&target),
        Commands::Unmute { target } => ctx.unmute(&target),
        Commands::ToggleMute { target } => ctx.toggle_mute(&target),
        Commands::Vol { target, percent } => ctx.set_volume(&target, percent),
        Commands::DumpConfig => ctx.dump_config(),
        Commands::ReloadConfig => ctx.reload_config(),
        Commands::ToggleStream => ctx.toggle_stream(),
        Commands::ToggleRecord => ctx.toggle_record(),
        // Already handled above
        Commands::Init
        | Commands::ValidateConfig
        | Commands::Server { .. }
        | Commands::Tui
        | Commands::Service { .. } => unreachable!("local commands handled separately"),
    }
}
