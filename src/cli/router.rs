// CLI command router.

use std::path::PathBuf;

use crate::{
    cli::{
        args::{Cli, Commands, ServiceAction},
        client_commands::ProxyCtx,
    },
    config::{loader, model, paths, schema, writer},
    ipc::socket_path::resolve_server_socket_path,
    runtime::logger,
    server::{daemon, options::ServerOptions},
    service::{
        installer::{self, ServiceInstaller},
        systemd_user_service::{self, SystemctlRunner},
    },
    support::validation::{
        password_config_error_message, read_env_token, resolve_connection_password,
    },
};

pub fn run(cli: Cli) -> i32 {
    let config_path = cli.config.clone().or_else(paths::config_path);

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
        Some(cmd) => {
            logger::init_cli(&level);
            run_proxy(config_path, cmd, cli.json)
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
    if let Some(level) = cli.log_level.clone() {
        return level;
    }
    if let Some(rust_log) = read_env_token("RUST_LOG")
        && let Ok(level) = crate::cli::args::parse_log_level(&rust_log)
    {
        return level;
    }
    match &cli.command {
        Some(Commands::Server { .. }) => "info",
        _ => "warn",
    }
    .to_string()
}

fn resolve_socket_config(config_path: Option<&PathBuf>) -> Result<(PathBuf, u64), String> {
    if let Some(cp) = config_path {
        let (_, socket_path, refresh_interval_ms) = loader::load_or_default_with_runtime(cp)
            .map_err(|e| format!("failed to load config: {e}"))?;

        return Ok((socket_path, refresh_interval_ms));
    }

    if let Some(default_cp) = paths::config_path() {
        match loader::load_or_default_with_runtime(&default_cp) {
            Ok((_, socket_path, refresh_interval_ms)) => {
                return Ok((socket_path, refresh_interval_ms));
            }
            Err(error) => {
                return Err(format!("failed to load default config: {error}"));
            }
        }
    }

    let default_refresh_interval_ms = model::Config::default().ui.refresh_interval_ms;
    Ok((
        resolve_server_socket_path(None).map_err(|e| e.to_string())?,
        default_refresh_interval_ms,
    ))
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

    let (_config, warnings) = match crate::config::loader::load_with_warnings(&path) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

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

    let rt = match tokio_runtime("server") {
        Some(rt) => rt,
        None => return 1,
    };

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
    let port = prompt_port(&stdin, &mut buf)?;
    let password = loop {
        let value = prompt_line(
            &stdin,
            &mut buf,
            "OBS WebSocket password (blank = no auth)",
            "",
        )?;
        if let Err(error) = resolve_connection_password(Some(&value), "") {
            eprintln!("  {}", password_config_error_message(&error));
            continue;
        }
        break value;
    };
    schema::validate_connection_host(&host).map_err(|e| std::io::Error::other(e.to_string()))?;

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

fn prompt_port(stdin: &std::io::Stdin, buf: &mut String) -> std::io::Result<u16> {
    loop {
        let port_str = prompt_line(stdin, buf, "OBS port", "4455")?;
        match port_str.parse::<u16>() {
            Ok(0) => {
                eprintln!("  Port must be in range 1-65535.");
            }
            Ok(port) => return Ok(port),
            Err(_) => {
                eprintln!("  Invalid port, enter an integer 1-65535.");
            }
        }
    }
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
    let (socket_path, refresh_ms) = match resolve_socket_config(config_path.as_ref()) {
        Ok(values) => values,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let theme = resolve_tui_theme(config_path.as_ref());

    let rt = match tokio_runtime("TUI") {
        Some(rt) => rt,
        None => return 1,
    };

    match rt.block_on(crate::tui::app::run(&socket_path, refresh_ms, theme)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tui error: {e}");
            1
        }
    }
}

/// Best-effort resolution of the configured TUI theme (built-in id or a
/// `custom` palette); falls back to the default theme on any load error so
/// a bad/missing config never blocks launching the TUI (the error path is
/// already reported by `resolve_socket_config`).
fn resolve_tui_theme(config_path: Option<&PathBuf>) -> crate::tui::theme::Theme {
    let effective_path = config_path.cloned().or_else(paths::config_path);
    let config = effective_path
        .and_then(|cp| loader::load_or_default(&cp).ok())
        .unwrap_or_else(model::Config::default);
    let custom = config
        .ui
        .custom_theme
        .map(|c| crate::tui::theme::CustomThemeSpec {
            accent: c.accent,
            accent_alt: c.accent_alt,
            fg: c.fg,
            muted: c.muted,
            border: c.border,
            border_focus: c.border_focus,
            success: c.success,
            warning: c.warning,
            danger: c.danger,
            info: c.info,
            highlight_bg: c.highlight_bg,
            highlight_fg: c.highlight_fg,
        });
    crate::tui::theme::Theme::resolve(&config.ui.theme, custom.as_ref())
}

fn tokio_runtime(context: &str) -> Option<tokio::runtime::Runtime> {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => Some(rt),
        Err(e) => {
            eprintln!("error: failed to start async runtime for {context}: {e}");
            None
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
    let installer = ServiceInstaller::new(&runner, unit_path.clone());

    match action {
        ServiceAction::Install => {
            let exec = match resolve_service_exec_path() {
                Ok(exec) => exec,
                Err(message) => {
                    eprintln!("error: {message}");
                    return 1;
                }
            };
            if let Err(e) = installer.install(&exec) {
                eprintln!("error: {e}");
                return 1;
            }
            println!("Service installed at {}", unit_path.display());
            println!(
                "Enable with: {}",
                systemd_user_service::SYSTEMCTL_ENABLE_HINT
            );
            0
        }
        ServiceAction::Uninstall => {
            if let Err(e) = installer.uninstall() {
                eprintln!("error: {e}");
                return 1;
            }
            println!("Service uninstalled.");
            0
        }
        ServiceAction::Status => installer
            .status()
            .map(|out| {
                print!("{out}");
            })
            .map(|_| 0)
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                1
            }),
        ServiceAction::Start => service_action(&installer, "start"),
        ServiceAction::Stop => service_action(&installer, "stop"),
        ServiceAction::Restart => service_action(&installer, "restart"),
    }
}

fn resolve_service_exec_path() -> Result<std::path::PathBuf, String> {
    let exec =
        std::env::current_exe().map_err(|e| format!("could not determine executable path: {e}"))?;
    installer::validate_service_exec_path(&exec).map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::support::fs;
    use std::io::Write;

    fn with_rust_log_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os("RUST_LOG");
        if let Some(value) = value {
            unsafe { std::env::set_var("RUST_LOG", value) };
        } else {
            unsafe { std::env::remove_var("RUST_LOG") };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var("RUST_LOG", previous) },
            None => unsafe { std::env::remove_var("RUST_LOG") },
        }

        result
    }

    fn with_obsctl_config_env<R>(value: Option<&std::path::Path>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os("OBSCTL_CONFIG");
        if let Some(value) = value {
            unsafe { std::env::set_var("OBSCTL_CONFIG", value) };
        } else {
            unsafe { std::env::remove_var("OBSCTL_CONFIG") };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var("OBSCTL_CONFIG", previous) },
            None => unsafe { std::env::remove_var("OBSCTL_CONFIG") },
        }

        result
    }

    #[cfg(unix)]
    fn with_rust_log_env_os<R>(value: Option<std::ffi::OsString>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os("RUST_LOG");
        if let Some(value) = value {
            unsafe { std::env::set_var("RUST_LOG", value) };
        } else {
            unsafe { std::env::remove_var("RUST_LOG") };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var("RUST_LOG", previous) },
            None => unsafe { std::env::remove_var("RUST_LOG") },
        }

        result
    }

    #[test]
    fn effective_log_level_prefers_cli_verbose() {
        let cli = Cli {
            config: None,
            log_level: None,
            verbose: true,
            force: false,
            json: false,
            command: None,
        };
        assert_eq!(effective_log_level(&cli), "debug");
    }

    #[test]
    fn effective_log_level_uses_log_level_arg() {
        let cli = Cli {
            config: None,
            log_level: Some("error".to_string()),
            verbose: false,
            force: false,
            json: false,
            command: None,
        };
        assert_eq!(effective_log_level(&cli), "error");
    }

    #[test]
    fn effective_log_level_rejects_invalid_rust_log() {
        let cli = Cli {
            config: None,
            log_level: None,
            verbose: false,
            force: false,
            json: false,
            command: Some(Commands::Server { headless: false }),
        };
        with_rust_log_env(Some("verbose"), || {
            assert_eq!(effective_log_level(&cli), "info");
        });
    }

    #[cfg(unix)]
    #[test]
    fn effective_log_level_rejects_non_unicode_rust_log() {
        use std::os::unix::ffi::OsStringExt;

        let cli = Cli {
            config: None,
            log_level: None,
            verbose: false,
            force: false,
            json: false,
            command: Some(Commands::Tui),
        };
        with_rust_log_env_os(Some(std::ffi::OsString::from_vec(vec![0xff])), || {
            assert_eq!(effective_log_level(&cli), "warn");
        });
    }

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[test]
    fn service_exec_validator_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = installer::validate_service_exec_path(dir.path()).unwrap_err();
        assert!(err.to_string().contains("not a regular executable file"));
    }

    #[cfg(unix)]
    #[test]
    fn service_exec_validator_rejects_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("obsctl-temp-bin");
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        fs::secure_permissions(&file, 0o644).unwrap();
        let err = installer::validate_service_exec_path(&file).unwrap_err();
        assert!(err.to_string().contains("is not executable"));
    }

    #[cfg(unix)]
    #[test]
    fn service_exec_validator_rejects_world_or_group_writable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("obsctl-world-writable");
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        fs::secure_permissions(&file, 0o777).unwrap();
        let err = installer::validate_service_exec_path(&file).unwrap_err();
        assert!(err.to_string().contains("writable by group or other"));
    }

    #[cfg(unix)]
    #[test]
    fn service_exec_validator_rejects_insecure_special_mode_bits() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("obsctl-special-bits");
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        fs::secure_permissions(&file, 0o4755).unwrap();
        let err = installer::validate_service_exec_path(&file).unwrap_err();
        assert!(err.to_string().contains("insecure special mode bits"));
    }

    #[cfg(unix)]
    #[test]
    fn service_exec_validator_rejects_symlinked_file() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("obsctl-real-bin");
        let link = dir.path().join("obsctl-link-bin");
        let mut f = std::fs::File::create(&real).unwrap();
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        fs::secure_permissions(&real, 0o755).unwrap();
        symlink(&real, &link).unwrap();

        let err = installer::validate_service_exec_path(&link).unwrap_err();
        assert!(err.to_string().contains("symbolic link"));
    }

    #[cfg(unix)]
    #[test]
    fn service_exec_validator_rejects_control_characters() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let dir = tempfile::tempdir().unwrap();
        let mut name = b"obsctl".to_vec();
        name.extend(b"-bad\x0a-path");
        let path = dir
            .path()
            .join(std::path::PathBuf::from(OsString::from_vec(name)));

        let mut f = std::fs::File::create(&path).unwrap();
        use std::io::Write;
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        fs::secure_permissions(&path, 0o755).unwrap();

        let err = installer::validate_service_exec_path(&path).unwrap_err();
        assert!(err.to_string().contains("contains control characters"));
    }

    #[test]
    fn service_exec_validator_accepts_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("obsctl-temp-bin");
        let mut f = std::fs::File::create(&file).unwrap();
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        fs::secure_permissions(&file, 0o755).unwrap();

        let validated = installer::validate_service_exec_path(&file).unwrap();
        assert_eq!(validated, file);
    }

    #[test]
    fn resolve_socket_config_rejects_zero_refresh_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: 1\nui:\n  refresh_interval_ms: 0\n").unwrap();
        let err = resolve_socket_config(Some(&path)).unwrap_err();
        assert!(err.contains("refresh_interval_ms"));
    }

    #[test]
    fn resolve_socket_config_rejects_invalid_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\nconnection:\n  host: bad host\n  password: foo\n",
        )
        .unwrap();

        let err = resolve_socket_config(Some(&path)).unwrap_err();
        assert!(err.contains("config invalid"));
        assert!(err.contains("connection.host"));
    }

    #[test]
    fn resolve_socket_config_rejects_invalid_socket_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: 1\nserver:\n  socket_path: relative.sock\n").unwrap();

        let err = resolve_socket_config(Some(&path)).unwrap_err();
        assert!(err.contains("server.socket_path"));
    }

    #[test]
    fn resolve_socket_config_without_path_uses_defaults() {
        with_obsctl_config_env(None, || {
            let (socket_path, refresh_interval_ms) = resolve_socket_config(None).unwrap();
            assert!(socket_path.is_absolute());
            assert_eq!(
                socket_path.file_name().and_then(|name| name.to_str()),
                Some("obsctl.sock")
            );
            assert_eq!(
                refresh_interval_ms,
                model::Config::default().ui.refresh_interval_ms
            );
        });
    }

    #[test]
    fn resolve_socket_config_without_path_prefers_default_config() {
        let dir = tempfile::tempdir().unwrap();
        let socket_dir = dir.path().join("sockets");
        std::fs::create_dir(&socket_dir).unwrap();
        fs::secure_permissions(&socket_dir, 0o700).unwrap();
        let socket_path = socket_dir.join("obsctl.sock");
        let config_path = dir.path().join("config.yml");
        std::fs::write(
            &config_path,
            format!(
                "version: 1\nserver:\n  socket_path: {}\nui:\n  refresh_interval_ms: 321\n",
                socket_path.display()
            ),
        )
        .unwrap();

        with_obsctl_config_env(Some(config_path.as_path()), || {
            let (resolved_socket_path, refresh_interval_ms) = resolve_socket_config(None).unwrap();
            assert_eq!(resolved_socket_path, socket_path);
            assert_eq!(refresh_interval_ms, 321);
        });
    }
}

fn service_action(installer: &ServiceInstaller<'_>, verb: &str) -> i32 {
    let result = match verb {
        "start" => installer.start(),
        "stop" => installer.stop(),
        "restart" => installer.restart(),
        _ => unreachable!("unsupported verb"),
    };

    match result {
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
    let (socket_path, _) = match resolve_socket_config(config_path.as_ref()) {
        Ok(values) => values,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
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
        other => {
            eprintln!("error: command {other:?} is not supported for server proxying");
            1
        }
    }
}
