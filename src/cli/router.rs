// CLI command router.

use std::path::PathBuf;

use rust_i18n::t;

use crate::{
    cli::{
        args::{Cli, Commands, ServiceAction},
        client_commands::{self, ProxyCtx},
    },
    config::{loader, model, paths, schema, writer},
    domain::errors::ObsctlError,
    ipc::{protocol::CommandPayload, socket_path::resolve_server_socket_path},
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

/// Resolve the config file this launch will use.
///
/// Precedence, highest first:
///
/// 1. `--config` on the command line (already checked by clap's
///    `non_blank_path`: absolute, no traversal, no control characters);
/// 2. the `OBSCTL_CONFIG` environment variable (checked inside
///    [`paths::config_path`], which quietly falls back to the platform default
///    when the value is relative, a symlink, over-long, or not valid UTF-8);
/// 3. the platform default location, e.g. `~/.config/obsctl/config.yml`.
///
/// Because `paths::config_path()` already ends in `default_config_path()` on
/// every one of its rejection paths, the answer is `None` only when the
/// platform offers no config directory at all. That is why nothing downstream
/// re-applies any part of this chain: four callees used to end in their own
/// `.or_else(paths::config_path).or_else(paths::default_config_path)` that
/// could never change the value. Please do not put them back.
fn resolve_config_path(explicit: Option<PathBuf>) -> Option<PathBuf> {
    explicit.or_else(paths::config_path)
}

/// Everything resolved once when the process starts, then handed to whichever
/// command runs.
///
/// Launching the TUI used to parse the config file three separate times — once
/// to find `ui.locale`, once to find the socket path, and once to find the
/// theme — each with its own idea of what to do when the file was bad (ignore
/// it, fail the command, silently use defaults). Reading it once and passing
/// the result around means one precedence rule, one read, and one decision
/// about failure.
struct Startup {
    /// The config file in effect, or `None` when there is no config directory.
    config_path: Option<PathBuf>,
    /// Outcome of the single attempt to read that file.
    ///
    /// Kept as a `Result` rather than being resolved here because the commands
    /// genuinely disagree about whether an unusable config is fatal: `init`
    /// exists to *write* a config file and has to run without one, while the
    /// TUI and the proxy commands need the socket path it names. The `String`
    /// is the message the failing command prints.
    runtime: Result<StartupRuntime, String>,
}

/// The parts of a loaded config the commands actually use.
#[derive(Debug)]
struct StartupRuntime {
    config: model::Config,
    socket_path: PathBuf,
    refresh_interval_ms: u64,
}

impl Startup {
    fn resolve(config_path: Option<PathBuf>) -> Self {
        let runtime = match config_path.as_deref() {
            Some(path) => loader::load_or_default_with_runtime(path)
                .map(
                    |(config, socket_path, refresh_interval_ms)| StartupRuntime {
                        config,
                        socket_path,
                        refresh_interval_ms,
                    },
                )
                .map_err(|error| format!("failed to load config: {error}")),
            // No config file could be resolved at all, so the built-in
            // defaults plus the default socket location are the whole
            // configuration.
            None => resolve_server_socket_path(None)
                .map_err(|error| error.to_string())
                .map(|socket_path| {
                    let config = model::Config::default();
                    let refresh_interval_ms = config.ui.refresh_interval_ms;
                    StartupRuntime {
                        config,
                        socket_path,
                        refresh_interval_ms,
                    }
                }),
        };

        Self {
            config_path,
            runtime,
        }
    }

    /// The configured UI language, when the config could be read.
    ///
    /// A config that failed to load simply means "no override": which language
    /// the messages are in must never be the reason a command cannot run, and
    /// the command that actually needs the config reports the failure itself.
    fn config_locale(&self) -> Option<&str> {
        self.runtime.as_ref().ok()?.config.ui.locale.as_deref()
    }

    /// The loaded config, or the message explaining why there is none.
    fn runtime(&self) -> Result<&StartupRuntime, &String> {
        self.runtime.as_ref()
    }
}

pub fn run(cli: Cli) -> i32 {
    // Logging is installed before anything else, and in particular before
    // localization. `localization::init` warns through `tracing` when the
    // requested locale is unknown ("unsupported locale, falling back to en").
    // A `tracing` event emitted while no subscriber is installed is discarded,
    // so with the old order that warning could never reach the user: someone
    // setting `OBSCTL_LOCALE=fr` was silently given English with no
    // explanation. Installing the subscriber first is what makes the
    // diagnostic reachable.
    let level = effective_log_level(&cli);
    init_logging(cli.command.as_ref(), &level);

    let startup = Startup::resolve(resolve_config_path(cli.config.clone()));
    crate::localization::init(startup.config_path.as_deref(), startup.config_locale());

    match cli.command {
        None | Some(Commands::Tui) => run_tui(&startup),
        Some(Commands::Init) => run_init(startup.config_path.as_deref(), cli.force),
        Some(Commands::ValidateConfig) => run_validate_config(startup.config_path.as_deref()),
        Some(Commands::Server { headless }) => run_server(startup.config_path, headless),
        Some(Commands::Service { action }) => run_service(action),
        Some(cmd) => run_proxy(&startup, cmd, cli.json),
    }
}

/// Install the process-wide `tracing` subscriber for the mode being launched.
///
/// The daemon logs to a file as well as stderr, because its output has to
/// outlive the terminal it was started from and is also replayed to connected
/// clients over the `logs` IPC topic. Every other mode is a short-lived
/// foreground command, so stderr alone is enough.
fn init_logging(command: Option<&Commands>, level: &str) {
    match command {
        Some(Commands::Server { .. }) => logger::init_server(level, logger::default_log_path()),
        _ => logger::init_cli(level),
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

// ── Local commands ────────────────────────────────────────────────────────────

fn run_init(config_path: Option<&std::path::Path>, force: bool) -> i32 {
    let Some(path) = config_path else {
        return fail(t!("cli.init.no_config_dir"));
    };

    if path.exists() && !force {
        eprintln!("{}", t!("cli.init.already_exists", path = path.display()));
        // No `ObsctlError` variant names "the file is already there", and
        // adding one would change the public error table, so this reports the
        // unclassified local failure code by name rather than as a literal.
        return GENERIC_LOCAL_FAILURE_EXIT_CODE;
    }

    match writer::write_default(path) {
        Ok(()) => {
            println!("{}", t!("cli.init.success", path = path.display()));
            0
        }
        Err(e) => fail(e),
    }
}

fn run_validate_config(config_path: Option<&std::path::Path>) -> i32 {
    let Some(path) = config_path else {
        {
            // The wording is this command's own localized line; only the exit
            // code comes from the error value. `ObsctlError::exit_code()` is
            // the documented local classification (README "Exit Codes"), so
            // "no usable config means 2" is decided in exactly one place
            // instead of being re-typed as a literal here.
            eprintln!(
                "{}",
                t!("common.error", message = t!("cli.validate.no_config_path"))
            );
            return ObsctlError::ConfigNotFound(t!("cli.validate.no_config_path").to_string())
                .exit_code();
        }
    };

    let (_config, warnings) = match crate::config::loader::load_with_warnings(path) {
        Ok(result) => result,
        Err(error) => {
            // Previously this returned a hard-coded 2 for every load failure,
            // including the ones the loader reports as `ObsctlError::Io`
            // (an unreadable file, a permissions problem) which the local table
            // classifies as a generic failure, 1. Asking the error for its own
            // code makes the two agree.
            eprintln!("{}", t!("common.error", message = error));
            return error.exit_code();
        }
    };

    for w in &warnings {
        eprintln!("{}", t!("common.warning", message = &w.0));
    }
    if warnings.is_empty() {
        println!("{}", t!("cli.validate.valid"));
    } else {
        println!(
            "{}",
            t!("cli.validate.valid_with_warnings", count = warnings.len())
        );
    }
    0
}

fn run_server(config_path: Option<PathBuf>, headless: bool) -> i32 {
    // The path the daemon will use was already settled by `resolve_config_path`,
    // so first-time setup writes to the same file the daemon then reads.
    if let Some(ref path) = config_path
        && !path.exists()
        && !headless
        && let Err(error) = first_time_setup(path)
    {
        eprintln!("{}", t!("cli.setup.failed", error = error.to_string()));
        return ObsctlError::Io(error).exit_code();
    }

    let rt = match tokio_runtime("server") {
        Ok(rt) => rt,
        Err(error) => return error.exit_code(),
    };

    let options = ServerOptions {
        headless,
        config_path,
    };

    rt.block_on(daemon::run(options))
}

/// Interactively prompt for OBS connection details and write a minimal config file.
fn first_time_setup(config_path: &std::path::Path) -> std::io::Result<()> {
    eprintln!(
        "{}",
        t!("cli.setup.no_config_found", path = config_path.display())
    );
    eprint!("{}", t!("cli.setup.create_minimal"));

    let stdin = std::io::stdin();
    let mut buf = String::new();

    let host = prompt_line(&stdin, &mut buf, &t!("cli.setup.prompt_host"), "127.0.0.1")?;
    let port = prompt_port(&stdin, &mut buf)?;
    let password = loop {
        let value = prompt_line(&stdin, &mut buf, &t!("cli.setup.prompt_password"), "")?;
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

    eprint!("{}", t!("cli.setup.written", path = config_path.display()));
    Ok(())
}

fn prompt_port(stdin: &std::io::Stdin, buf: &mut String) -> std::io::Result<u16> {
    loop {
        let port_str = prompt_line(stdin, buf, &t!("cli.setup.prompt_port"), "4455")?;
        match port_str.parse::<u16>() {
            Ok(0) => {
                eprintln!("{}", t!("cli.setup.port_out_of_range"));
            }
            Ok(port) => return Ok(port),
            Err(_) => {
                eprintln!("{}", t!("cli.setup.port_invalid"));
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

fn run_tui(startup: &Startup) -> i32 {
    let runtime = match startup.runtime() {
        Ok(runtime) => runtime,
        Err(error) => return fail(error),
    };

    let mut options = tui_appearance(&runtime.config);
    options.refresh_ms = runtime.refresh_interval_ms;
    options.config_path = startup.config_path.clone();

    let rt = match tokio_runtime("TUI") {
        Ok(rt) => rt,
        Err(error) => return error.exit_code(),
    };

    match rt.block_on(crate::tui::app::run(&runtime.socket_path, options)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}", t!("cli.tui.error", error = e));
            1
        }
    }
}

/// Translate the `ui` section of an already-loaded config into the TUI's
/// appearance and input options: built-in theme id or a `custom` palette,
/// icons, mouse, command-palette prefix.
///
/// This used to read the config file itself and silently substitute defaults
/// when the read failed — the third read of the same file in one launch, and a
/// swallowed error that could disagree with what the rest of the launch had
/// decided. It is now a plain transformation with no I/O: the caller has
/// already read the file once and reported any problem. `refresh_ms` and
/// `config_path` are filled in by the caller, which has them to hand.
fn tui_appearance(config: &model::Config) -> crate::tui::app::TuiOptions {
    let custom = config
        .ui
        .custom_theme
        .clone()
        .map(|c| crate::tui::theme::CustomThemeSpec {
            bg: c.bg,
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
    crate::tui::app::TuiOptions {
        theme: crate::tui::theme::Theme::resolve(&config.ui.theme, custom.as_ref()),
        show_icons: config.ui.show_icons,
        advanced_ui: config.ui.advanced_ui,
        mouse: config.ui.mouse,
        palette_prefix: config
            .ui
            .command_palette_prefix
            .chars()
            .next()
            .filter(|c| crate::domain::parser::PALETTE_PREFIXES.contains(c))
            .unwrap_or(crate::domain::parser::DEFAULT_PALETTE_PREFIX),
        ..crate::tui::app::TuiOptions::default()
    }
}

/// Build the async runtime the TUI and the daemon both need.
///
/// The failure message names which of the two was being started, so it is
/// built here where `context` is in hand rather than at the call site. What
/// travels back to the caller is an `ObsctlError`, so the exit code comes from
/// the documented local classification instead of a literal `1` repeated at
/// each call site.
fn tokio_runtime(context: &str) -> Result<tokio::runtime::Runtime, ObsctlError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            eprintln!(
                "{}",
                t!(
                    "common.error",
                    message = t!(
                        "cli.runtime.async_start_failed",
                        context = context,
                        error = error.to_string()
                    )
                )
            );
            ObsctlError::Io(error)
        })
}

// ── Service commands ──────────────────────────────────────────────────────────

/// Run one of the three systemctl verbs that behave alike: do it, then print
/// either the matching success line or the error.
///
/// Takes the `ServiceAction` the caller already has rather than a `"start"` /
/// `"stop"` / `"restart"` string. The string version needed a catch-all arm
/// that panicked on any other word; with the enum, the compiler rejects an
/// unhandled variant instead and there is no panic to reach.
fn service_action(installer: &ServiceInstaller<'_>, action: ServiceAction) -> i32 {
    let (result, done_key) = match action {
        ServiceAction::Start => (installer.start(), "cli.service.action_started"),
        ServiceAction::Stop => (installer.stop(), "cli.service.action_stopped"),
        ServiceAction::Restart => (installer.restart(), "cli.service.action_restarted"),
        // The caller routes the other variants to their own handlers; they
        // have nothing in common with these three beyond taking a unit name.
        ServiceAction::Install | ServiceAction::Uninstall | ServiceAction::Status => {
            return fail(t!(
                "cli.proxy.unsupported_command",
                command = format!("{action:?}")
            ));
        }
    };

    match result {
        Ok(_) => {
            println!("{}", t!(done_key));
            0
        }
        Err(e) => fail(e),
    }
}

/// Exit code for a local failure the error table has no specific class for.
///
/// It is the same `1` that `ObsctlError::exit_code()` assigns to its
/// unclassified variants (`Io`, `ServiceInstallFailed`, ...). Named once here
/// so the handful of local paths that mean "generic failure" cannot drift apart
/// from each other or from the README's Exit Codes table.
const GENERIC_LOCAL_FAILURE_EXIT_CODE: i32 = 1;

/// Print `message` as an error and hand back the generic failure exit code.
///
/// Used by the local (non-proxy) commands, which all report failure the same
/// way; proxy commands go through `PublicErrorCode::exit_code` instead.
fn fail(message: impl std::fmt::Display) -> i32 {
    eprintln!("{}", t!("common.error", message = message));
    GENERIC_LOCAL_FAILURE_EXIT_CODE
}

fn run_service(action: ServiceAction) -> i32 {
    let runner = SystemctlRunner;
    let unit_path = match systemd_user_service::unit_file_path() {
        Some(p) => p,
        None => return fail(t!("cli.service.no_unit_dir")),
    };
    let installer = ServiceInstaller::new(&runner, unit_path.clone());

    match action {
        ServiceAction::Install => {
            let exec = match resolve_service_exec_path() {
                Ok(exec) => exec,
                Err(message) => return fail(message),
            };
            if let Err(e) = installer.install(&exec) {
                return fail(e);
            }
            println!(
                "{}",
                t!("cli.service.installed", path = unit_path.display())
            );
            println!(
                "{}",
                t!(
                    "cli.service.enable_hint",
                    hint = systemd_user_service::SYSTEMCTL_ENABLE_HINT
                )
            );
            0
        }
        ServiceAction::Uninstall => {
            if let Err(e) = installer.uninstall() {
                return fail(e);
            }
            println!("{}", t!("cli.service.uninstalled"));
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
        start_stop_restart => service_action(&installer, start_stop_restart),
    }
}

fn resolve_service_exec_path() -> Result<std::path::PathBuf, String> {
    let exec =
        std::env::current_exe().map_err(|e| format!("could not determine executable path: {e}"))?;
    installer::validate_service_exec_path(&exec).map_err(|e| e.to_string())
}

// ── Proxy commands ────────────────────────────────────────────────────────────

/// What a CLI subcommand turns into on the IPC wire.
///
/// Three outcomes are possible and the compiler makes the caller handle all
/// three, which is why this is an enum rather than an `Option`: a command to
/// send, arguments that failed validation before anything was sent, and the
/// modes the router runs itself and never proxies.
enum ProxyRequest {
    /// The payload to send, plus how to render a successful result.
    Send(CommandPayload, fn(&serde_json::Value)),
    /// The arguments did not survive validation, so nothing is sent.
    Invalid(ObsctlError),
    /// Handled locally by `run`; it never reaches the daemon.
    NotProxied,
}

/// Map a CLI subcommand onto the daemon command it stands for.
///
/// This replaces a layer of sixteen one-line `ProxyCtx` methods that this same
/// match used to call — each of which only re-stated the name mapping the match
/// already performs. Adding a proxy command now means three edits (the clap
/// `Commands` variant, an arm here, and the `ipc::protocol::ServerCommand`
/// variant) instead of four.
///
/// Argument validation happens here, before a connection is opened, so a bad
/// target or an out-of-range percentage never becomes a request.
fn proxy_payload(cmd: &Commands) -> ProxyRequest {
    use crate::ipc::protocol::ServerCommand;

    /// Validate a target name, then build a payload that carries it.
    fn with_target(command: ServerCommand, target: &str) -> ProxyRequest {
        match client_commands::sanitize_target_arg(target) {
            Ok(target) => ProxyRequest::Send(
                CommandPayload::with_target(command, &target),
                client_commands::print_result_message,
            ),
            Err(error) => ProxyRequest::Invalid(error),
        }
    }

    /// Build a payload with no arguments.
    fn simple(command: ServerCommand) -> ProxyRequest {
        ProxyRequest::Send(
            CommandPayload::simple(command),
            client_commands::print_result_message,
        )
    }

    match cmd {
        // `status` is the one command whose result is a structure rather than a
        // sentence, so it gets its own renderer.
        Commands::Status => ProxyRequest::Send(
            CommandPayload::simple(ServerCommand::GetSnapshot),
            client_commands::print_status_json,
        ),
        Commands::ObsStatus => simple(ServerCommand::GetObsStatus),
        Commands::ServerStatus => simple(ServerCommand::GetServerStatus),
        Commands::Reconnect => simple(ServerCommand::ReconnectObs),
        Commands::ShutdownServer => simple(ServerCommand::ShutdownServer),
        Commands::DumpConfig => simple(ServerCommand::DumpConfig),
        Commands::ReloadConfig => simple(ServerCommand::ReloadConfig),
        Commands::ToggleStream => simple(ServerCommand::ToggleStream),
        Commands::ToggleRecord => simple(ServerCommand::ToggleRecord),
        Commands::Scene { target } => with_target(ServerCommand::SetScene, target),
        Commands::Profile { target } => with_target(ServerCommand::SetProfile, target),
        Commands::Collection { target } => with_target(ServerCommand::SetSceneCollection, target),
        Commands::Mute { target } => with_target(ServerCommand::Mute, target),
        Commands::Unmute { target } => with_target(ServerCommand::Unmute, target),
        Commands::ToggleMute { target } => with_target(ServerCommand::ToggleMute, target),
        Commands::Vol { target, percent } => {
            // clap already limits this argument to 0..=100, but
            // `CommandPayload::set_volume` is reachable from tests and from any
            // future caller, so the range is checked here as well rather than
            // being trusted to the parser.
            if *percent > 100 {
                return ProxyRequest::Invalid(ObsctlError::CommandParseError(
                    "percent must be 0-100".to_string(),
                ));
            }
            match client_commands::sanitize_target_arg(target) {
                Ok(target) => ProxyRequest::Send(
                    CommandPayload::set_volume(&target, *percent),
                    client_commands::print_result_message,
                ),
                Err(error) => ProxyRequest::Invalid(error),
            }
        }
        // Listed rather than caught by a wildcard: these are the modes the
        // router handles itself before it gets here, so a newly added
        // subcommand fails to compile until it is routed somewhere, instead of
        // silently reaching users as "unsupported command".
        Commands::Init
        | Commands::ValidateConfig
        | Commands::Server { .. }
        | Commands::Tui
        | Commands::Service { .. } => ProxyRequest::NotProxied,
    }
}

fn run_proxy(startup: &Startup, cmd: Commands, json_output: bool) -> i32 {
    // The config is checked before the subcommand is examined, which is the
    // order this function has always used: if the config cannot be loaded, the
    // user hears about that first, whatever they were trying to run.
    let socket_path = match startup.runtime() {
        Ok(runtime) => runtime.socket_path.clone(),
        Err(error) => return fail(error),
    };
    let ctx = ProxyCtx {
        socket_path,
        json_output,
    };

    match proxy_payload(&cmd) {
        ProxyRequest::Send(payload, render) => ctx.run_proxy_with(payload, render),
        ProxyRequest::Invalid(error) => ctx.emit_local_error(&error),
        ProxyRequest::NotProxied => fail(t!(
            "cli.proxy.unsupported_command",
            command = format!("{cmd:?}")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::fs;
    use std::io::Write;

    use crate::support::validation::test_env;

    fn with_rust_log_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        test_env::with_env_var("RUST_LOG", value, f)
    }

    fn with_obsctl_config_env<R>(value: Option<&std::path::Path>, f: impl FnOnce() -> R) -> R {
        test_env::with_env_var_os("OBSCTL_CONFIG", value.map(|p| p.as_os_str().to_owned()), f)
    }

    #[cfg(unix)]
    fn with_rust_log_env_os<R>(value: Option<std::ffi::OsString>, f: impl FnOnce() -> R) -> R {
        test_env::with_env_var_os("RUST_LOG", value, f)
    }

    /// A `tracing` writer that keeps everything written to it in memory.
    ///
    /// `tracing` events are dropped when no subscriber is installed, so the
    /// only way to show that a warning is actually reachable is to install a
    /// subscriber and read back what it recorded. Cloning shares one buffer:
    /// the subscriber gets one handle and the test keeps another to inspect.
    #[derive(Clone, Default)]
    struct SharedBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl SharedBuffer {
        fn contents(&self) -> String {
            let bytes = self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl std::io::Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuffer {
        type Writer = SharedBuffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test]
    fn unsupported_locale_warning_reaches_an_installed_subscriber() {
        let buffer = SharedBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .finish();

        // `with_default` installs the subscriber for this thread only, so the
        // test does not fight the process-wide subscriber `run()` installs.
        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(
                crate::localization::resolve_locale(Some("fr".to_string()), None),
                "en"
            );
        });

        assert!(
            buffer.contents().contains("unsupported locale"),
            "resolve_locale must report the fallback through tracing; got: {}",
            buffer.contents()
        );
    }

    #[test]
    fn init_logging_installs_a_subscriber_before_localization_runs() {
        // `run()` calls `init_logging` and only then `init_localization`. Both
        // install process-wide state that can only be set once, so this test
        // checks the property that ordering exists to guarantee: after logging
        // init, a `tracing` warning has somewhere to go. `try_init` inside
        // `logger::init_cli` is a no-op if another test already installed a
        // subscriber, which is why the assertion is about a subscriber being
        // present rather than about this call being the one that set it.
        init_logging(Some(&Commands::Tui), "warn");
        assert!(
            tracing::dispatcher::has_been_set(),
            "a global tracing subscriber must exist before localization warns"
        );
    }

    /// A `ProxyCtx` that can report a failure but could never connect: these
    /// tests assert that bad arguments are refused without a daemon.
    fn offline_proxy_ctx() -> ProxyCtx {
        ProxyCtx {
            socket_path: std::path::PathBuf::from("/tmp/obsctl-test.sock"),
            json_output: false,
        }
    }

    #[test]
    fn proxy_payload_rejects_percent_out_of_range_without_server() {
        for percent in [101u8, 255] {
            let request = proxy_payload(&Commands::Vol {
                target: "Mic".to_string(),
                percent,
            });
            match request {
                ProxyRequest::Invalid(error) => {
                    assert_eq!(offline_proxy_ctx().emit_local_error(&error), 5);
                }
                _ => panic!("percent {percent} must not become a request"),
            }
        }
    }

    #[test]
    fn proxy_payload_rejects_invalid_target_without_server() {
        let bad_targets = [
            Commands::Mute {
                target: "Bad\nTarget".to_string(),
            },
            Commands::Scene {
                target: "   ".to_string(),
            },
        ];

        for command in bad_targets {
            match proxy_payload(&command) {
                ProxyRequest::Invalid(error) => {
                    assert_eq!(offline_proxy_ctx().emit_local_error(&error), 5);
                }
                _ => panic!("{command:?} must not become a request"),
            }
        }
    }

    #[test]
    fn proxy_payload_leaves_locally_handled_modes_unproxied() {
        for command in [
            Commands::Init,
            Commands::ValidateConfig,
            Commands::Tui,
            Commands::Server { headless: true },
        ] {
            assert!(
                matches!(proxy_payload(&command), ProxyRequest::NotProxied),
                "{command:?} is run by the router, not sent to the daemon"
            );
        }
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

    /// Build a `Startup` from a config file written for the test.
    fn startup_for(path: &std::path::Path) -> Startup {
        Startup::resolve(Some(path.to_path_buf()))
    }

    #[test]
    fn startup_rejects_zero_refresh_interval() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: 1\nui:\n  refresh_interval_ms: 0\n").unwrap();
        let err = startup_for(&path).runtime.unwrap_err();
        assert!(err.contains("refresh_interval_ms"));
    }

    #[test]
    fn startup_rejects_invalid_host() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(
            &path,
            "version: 1\nconnection:\n  host: bad host\n  password: foo\n",
        )
        .unwrap();

        let err = startup_for(&path).runtime.unwrap_err();
        assert!(err.contains("config invalid"));
        assert!(err.contains("connection.host"));
    }

    #[test]
    fn startup_rejects_invalid_socket_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: 1\nserver:\n  socket_path: relative.sock\n").unwrap();

        let err = startup_for(&path).runtime.unwrap_err();
        assert!(err.contains("server.socket_path"));
    }

    #[test]
    fn startup_without_any_config_path_uses_defaults() {
        let startup = Startup::resolve(None);
        let runtime = startup.runtime().unwrap();
        assert!(runtime.socket_path.is_absolute());
        assert_eq!(
            runtime
                .socket_path
                .file_name()
                .and_then(|name| name.to_str()),
            Some("obsctl.sock")
        );
        assert_eq!(
            runtime.refresh_interval_ms,
            model::Config::default().ui.refresh_interval_ms
        );
    }

    /// `--config` wins over `OBSCTL_CONFIG`, which wins over the platform
    /// default. This is the precedence rule `resolve_config_path` documents;
    /// it is asserted here because it used to be spread over four callees that
    /// each re-applied a piece of it.
    #[test]
    fn config_path_precedence_prefers_the_explicit_flag_then_the_env_var() {
        let dir = tempfile::tempdir().unwrap();
        let from_env = dir.path().join("from-env.yml");
        let from_flag = dir.path().join("from-flag.yml");

        with_obsctl_config_env(Some(from_env.as_path()), || {
            assert_eq!(
                resolve_config_path(Some(from_flag.clone())),
                Some(from_flag.clone())
            );
            assert_eq!(resolve_config_path(None), Some(from_env.clone()));
        });
    }

    /// With no `--config`, the config named by `OBSCTL_CONFIG` is the one read,
    /// and the socket path and refresh interval come from it.
    #[test]
    fn startup_without_explicit_path_reads_the_env_config() {
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
            let startup = Startup::resolve(resolve_config_path(None));
            assert_eq!(startup.config_path.as_deref(), Some(config_path.as_path()));
            let runtime = startup.runtime().unwrap();
            assert_eq!(runtime.socket_path, socket_path);
            assert_eq!(runtime.refresh_interval_ms, 321);
        });
    }

    /// A config that cannot be read must not decide the language: the command
    /// that needs the config is the one that reports the failure.
    #[test]
    fn startup_locale_is_absent_when_the_config_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: [\n").unwrap();

        let startup = startup_for(&path);
        assert!(startup.runtime().is_err());
        assert_eq!(startup.config_locale(), None);
    }

    #[test]
    fn startup_exposes_the_configured_locale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "version: 1\nui:\n  locale: uk\n").unwrap();

        assert_eq!(startup_for(&path).config_locale(), Some("uk"));
    }
}
