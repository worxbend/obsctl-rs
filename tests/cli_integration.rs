// CLI integration tests.

use std::sync::Arc;

use assert_cmd::Command;
use obsctl_rs::ipc::{
    protocol::{CommandPayload, ErrorPayload, ServerMessage},
    session::{BroadcastHub, CommandDispatch},
    unix_server::IpcServer,
};
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

fn obsctl() -> Command {
    Command::cargo_bin("obsctl").unwrap()
}

fn temp_config(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("config.yml")
}

// ── init ─────────────────────────────────────────────────────────────────────

#[test]
fn init_creates_config() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);

    obsctl()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .success()
        .stdout(contains("Initialized config"));

    assert!(path.exists(), "config file should exist after init");
}

#[test]
fn init_fails_without_force_when_config_exists() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);

    // First init succeeds.
    obsctl()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .success();

    // Second init without --force fails.
    obsctl()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .failure()
        .stderr(contains("--force"));
}

#[test]
fn init_force_overwrites_existing_config() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);

    obsctl()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .success();

    obsctl()
        .args(["--config", path.to_str().unwrap(), "--force", "init"])
        .assert()
        .success();
}

// ── validate-config ───────────────────────────────────────────────────────────

#[test]
fn validate_config_passes_for_default() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);

    obsctl()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .success();

    obsctl()
        .env("OBS_WEBSOCKET_PASSWORD", "testpassword")
        .args(["--config", path.to_str().unwrap(), "validate-config"])
        .assert()
        .success()
        .stdout(contains("valid"));
}

#[test]
fn validate_config_fails_for_missing_file() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);

    obsctl()
        .args(["--config", path.to_str().unwrap(), "validate-config"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn validate_config_fails_for_bad_yaml() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);
    std::fs::write(&path, "version: [\n").unwrap();

    obsctl()
        .args(["--config", path.to_str().unwrap(), "validate-config"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn validate_config_rejects_bad_version() {
    let dir = TempDir::new().unwrap();
    let path = temp_config(&dir);
    let yaml = "version: 99\n";
    std::fs::write(&path, yaml).unwrap();

    obsctl()
        .args(["--config", path.to_str().unwrap(), "validate-config"])
        .assert()
        .failure()
        .code(2);
}

// ── proxy commands without server ─────────────────────────────────────────────

fn config_with_socket(dir: &TempDir) -> std::path::PathBuf {
    let sock = dir.path().join("obsctl-nonexistent-test.sock");
    let path = temp_config(dir);
    let yaml = format!("version: 1\nserver:\n  socket_path: {}\n", sock.display());
    std::fs::write(&path, yaml).unwrap();
    path
}

#[test]
fn status_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    obsctl()
        .args(["--config", config.to_str().unwrap(), "status"])
        .assert()
        .failure()
        .code(3)
        .stderr(
            contains("not running")
                .or(contains("unavailable"))
                .or(contains("failed")),
        );
}

#[test]
fn obs_status_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    obsctl()
        .args(["--config", config.to_str().unwrap(), "obs-status"])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn server_status_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    obsctl()
        .args(["--config", config.to_str().unwrap(), "server-status"])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn scene_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    obsctl()
        .args(["--config", config.to_str().unwrap(), "scene", "Main"])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn mute_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    obsctl()
        .args(["--config", config.to_str().unwrap(), "mute", "Mic"])
        .assert()
        .failure()
        .code(3);
}

#[test]
fn scene_profile_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    // Every form reaches the daemon, so every form must fail the same way when
    // there is no daemon. That includes the argument-less one: it asks the
    // daemon which profile is active rather than answering from the config
    // file, so "no daemon" is a failure there too, not "nothing to do".
    for args in [
        vec!["scene-profile", "streaming"],
        vec!["scene-profile"],
        vec!["scene-profile", "--off"],
        vec!["scene-profile", "--delete", "night"],
        vec!["scene-profiles"],
    ] {
        let mut command = obsctl();
        command.args(["--config", config.to_str().unwrap()]);
        command.args(&args);
        command.assert().failure().code(3);
    }
}

#[test]
fn vol_without_server_exits_3() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    obsctl()
        .args(["--config", config.to_str().unwrap(), "vol", "Mic", "70"])
        .assert()
        .failure()
        .code(3);
}

// ── help and version ──────────────────────────────────────────────────────────

#[test]
fn help_flag_works() {
    obsctl()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("obsctl"));
}

#[test]
fn version_flag_works() {
    obsctl().arg("--version").assert().success();
}

#[test]
fn volume_alias_works() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    // `volume` is an alias for `vol`, should fail with exit 3 (no server), not 5 (parse error).
    obsctl()
        .args(["--config", config.to_str().unwrap(), "volume", "Mic", "50"])
        .assert()
        .failure()
        .code(3);
}

// ── --json flag ───────────────────────────────────────────────────────────────

#[derive(Clone)]
enum FakeIpcReply {
    Success(serde_json::Value),
    Error {
        code: String,
        message: String,
    },
    /// Reply with a description of the command that was received.
    ///
    /// This is how a test can tell *which* daemon command a subcommand turned
    /// into: the payload comes off the real wire, built by the real CLI, so no
    /// hand-written fixture can drift away from what is actually sent.
    Echo,
}

impl FakeIpcReply {
    fn success(response: serde_json::Value) -> Self {
        Self::Success(response)
    }

    fn error(code: &str, message: &str) -> Self {
        Self::Error {
            code: code.to_string(),
            message: message.to_string(),
        }
    }

    fn into_response(self, id: String, payload: &CommandPayload) -> ServerMessage {
        match self {
            Self::Success(result) => ServerMessage::Response {
                id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Self::Echo => ServerMessage::Response {
                id,
                ok: true,
                result: Some(serde_json::json!({
                    // `message` is what the CLI prints without `--json`, so the
                    // command name is readable from plain stdout too.
                    "message": payload.name,
                    "command": payload.name,
                    "args": payload.args,
                })),
                error: None,
            },
            Self::Error { code, message } => ServerMessage::Response {
                id,
                ok: false,
                result: None,
                error: Some(ErrorPayload::from_code(code, message)),
            },
        }
    }
}

/// Starts a fake IPC server in a background thread that responds to every
/// command with a fixed payload. Returns the TempDir (must be kept
/// alive) and the socket path written into a config file in that dir.
fn start_fake_ipc_server_with_config(response: serde_json::Value) -> (TempDir, std::path::PathBuf) {
    start_fake_ipc_server_with_reply(FakeIpcReply::success(response))
}

fn start_fake_ipc_server_with_reply(reply: FakeIpcReply) -> (TempDir, std::path::PathBuf) {
    start_fake_ipc_server_with_reply_and_log(reply, None)
}

fn start_fake_ipc_server_with_reply_and_log(
    reply: FakeIpcReply,
    log: Option<RecordedCommands>,
) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("fake.sock");
    let config_path = dir.path().join("config.yml");

    let yaml = format!(
        "version: 1\nserver:\n  socket_path: {}\n",
        socket_path.display()
    );
    std::fs::write(&config_path, yaml).unwrap();

    let socket_path_bg = socket_path.clone();
    let reply_bg = reply.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let hub = Arc::new(BroadcastHub::new());
            let (cmd_tx, mut cmd_rx) = mpsc::channel::<CommandDispatch>(64);
            let (_shutdown_tx, shutdown_rx) = watch::channel(false);
            let server = IpcServer::bind(&socket_path_bg, Arc::clone(&hub)).unwrap();
            let _ = ready_tx.send(());
            tokio::spawn(async move { server.run(cmd_tx, shutdown_rx).await });
            while let Some(dispatch) = cmd_rx.recv().await {
                // Recorded before the reply goes out, so a test that reads the
                // log after the CLI has exited always sees a complete list.
                if let Some(log) = &log {
                    log.record(&dispatch.payload.name);
                }
                let msg = reply_bg
                    .clone()
                    .into_response(dispatch.id.clone(), &dispatch.payload);
                let _ = dispatch.reply.send(msg);
            }
        });
    });

    ready_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("fake IPC server should bind");

    (dir, config_path)
}

/// The names of the daemon commands a fake server has been sent, in order.
///
/// Handed back by `start_recording_fake_ipc_server` so a test can assert on
/// what did *not* cross the socket as well as on what did.
#[derive(Clone, Default)]
struct RecordedCommands(Arc<std::sync::Mutex<Vec<String>>>);

impl RecordedCommands {
    fn record(&self, name: &str) {
        self.0.lock().unwrap().push(name.to_string());
    }

    fn recorded(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Like `start_fake_ipc_server_with_reply(FakeIpcReply::Echo)`, but keeps a log
/// of every command it answered.
fn start_recording_fake_ipc_server() -> (TempDir, std::path::PathBuf, RecordedCommands) {
    let commands = RecordedCommands::default();
    let (dir, config_path) =
        start_fake_ipc_server_with_reply_and_log(FakeIpcReply::Echo, Some(commands.clone()));
    (dir, config_path, commands)
}

fn parse_json_stdout(stdout: &[u8]) -> serde_json::Value {
    let stdout_str = String::from_utf8(stdout.to_vec()).unwrap();
    serde_json::from_str(stdout_str.trim()).expect("stdout should be valid JSON")
}

#[test]
fn json_flag_wraps_status_success_in_envelope() {
    let response = serde_json::json!({
        "connected": true,
        "current_scene": "Main",
        "message": "snapshot ok"
    });
    let (_dir, config_path) = start_fake_ipc_server_with_config(response.clone());

    let assert = obsctl()
        .args([
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "status",
        ])
        .assert()
        .success()
        .stderr(predicates::str::is_empty());

    let parsed = parse_json_stdout(&assert.get_output().stdout);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["result"], response);
    assert_eq!(parsed["error"], serde_json::Value::Null);
    assert_eq!(parsed["exit_code"], 0);
}

#[test]
fn json_flag_wraps_obs_status_success_in_envelope() {
    let response = serde_json::json!({
        "connected": false,
        "obs_studio_version": null,
        "message": "obs status ok"
    });
    let (_dir, config_path) = start_fake_ipc_server_with_config(response.clone());

    let output = obsctl()
        .args([
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "obs-status",
        ])
        .assert()
        .success()
        .stderr(predicates::str::is_empty())
        .get_output()
        .stdout
        .clone();

    let parsed = parse_json_stdout(&output);
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["result"], response);
    assert_eq!(parsed["error"], serde_json::Value::Null);
    assert_eq!(parsed["exit_code"], 0);
}

#[test]
fn json_flag_wraps_scene_mute_and_volume_successes_in_envelope() {
    let commands = [
        vec!["scene", "Main"],
        vec!["mute", "Mic"],
        vec!["vol", "Mic", "70"],
    ];

    for command in commands {
        let response = serde_json::json!({ "message": "ok" });
        let (_dir, config_path) = start_fake_ipc_server_with_config(response.clone());
        let mut args = vec!["--json", "--config", config_path.to_str().unwrap()];
        args.extend(command);

        let assert = obsctl()
            .args(args)
            .assert()
            .success()
            .stderr(predicates::str::is_empty());

        let parsed = parse_json_stdout(&assert.get_output().stdout);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["result"], response);
        assert_eq!(parsed["error"], serde_json::Value::Null);
        assert_eq!(parsed["exit_code"], 0);
    }
}

/// One subcommand, four daemon commands. An echoing fake daemon is what makes
/// the distinction observable — the assertion is on the command the CLI
/// actually put on the wire, not on a fixture describing it.
///
/// The case that matters is the argument-less one. It used to send
/// `clear_scene_profile`, which meant the most natural spelling of the question
/// "which scene profile is on?" destroyed the answer. It now sends
/// `list_scene_profiles`, the same read-only request as `scene-profiles`, and
/// switching off needs the explicit `--off`.
#[test]
fn each_written_form_of_scene_profile_sends_its_own_daemon_command() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::Echo);

    // The argument-less payloads arrive as an empty object: `args` is flattened
    // into the command object on the wire, so "no arguments" is "no extra keys".
    let no_args = serde_json::json!({});
    let cases = [
        (
            vec!["scene-profile", "streaming"],
            "set_scene_profile",
            serde_json::json!({ "target": "streaming" }),
        ),
        (
            vec!["scene-profile"],
            "list_scene_profiles",
            no_args.clone(),
        ),
        (
            vec!["scene-profile", "--off"],
            "clear_scene_profile",
            no_args.clone(),
        ),
        // `--clear` is the same flag under a second name, so it must reach the
        // same command rather than being parsed as a profile called "clear".
        (
            vec!["scene-profile", "--clear"],
            "clear_scene_profile",
            no_args.clone(),
        ),
        (
            vec!["scene-profile", "--delete", "night"],
            "delete_scene_profile",
            serde_json::json!({ "target": "night" }),
        ),
        (vec!["scene-profiles"], "list_scene_profiles", no_args),
    ];

    for (command, expected_name, expected_args) in cases {
        let mut args = vec!["--json", "--config", config_path.to_str().unwrap()];
        args.extend(command.iter().copied());

        let assert = obsctl().args(args).assert().success();
        let parsed = parse_json_stdout(&assert.get_output().stdout);
        assert_eq!(
            parsed["result"]["command"], expected_name,
            "{command:?} must send {expected_name}"
        );
        assert_eq!(
            parsed["result"]["args"], expected_args,
            "{command:?} sent the wrong arguments"
        );
    }
}

/// Asking which scene profile is active must not change which one is active.
///
/// The echoing daemon reports every command it is given, so this asserts on the
/// whole conversation rather than on one field of it: a single request went
/// over the socket, and it was the read-only one. A stray `clear_scene_profile`
/// sent alongside the listing — the shape of the old behaviour — would show up
/// here as a second recorded command.
#[test]
fn reading_which_scene_profile_is_active_sends_no_command_that_changes_it() {
    let (_dir, config_path, commands) = start_recording_fake_ipc_server();

    obsctl()
        .args(["--config", config_path.to_str().unwrap(), "scene-profile"])
        .assert()
        .success();

    assert_eq!(commands.recorded(), vec!["list_scene_profiles".to_string()]);
}

/// `--off` and `--delete` are two different instructions, and a name is a
/// third, so clap refuses any pair of them. The refusal happens while the
/// command line is being parsed, before a socket is opened, so the router never
/// has to invent a precedence rule for a combination with no sensible meaning.
///
/// The fake daemon is deliberately running and would answer any of these with
/// success: exit 2 rather than exit 0 is what proves the pair was rejected.
/// Exit 2 is clap's own usage-error code, shared with every other malformed
/// command line — no new error code was introduced for scene profiles.
#[test]
fn scene_profile_refuses_two_instructions_at_once() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::Echo);

    for command in [
        vec!["scene-profile", "streaming", "--off"],
        vec!["scene-profile", "streaming", "--delete", "night"],
        vec!["scene-profile", "--off", "--delete", "night"],
    ] {
        let mut args = vec!["--config", config_path.to_str().unwrap()];
        args.extend(command.iter().copied());

        obsctl()
            .args(args)
            .assert()
            .failure()
            .code(2)
            .stdout(predicates::str::is_empty())
            .stderr(contains("cannot be used with"));
    }
}

#[test]
fn json_flag_wraps_scene_profile_successes_in_envelope() {
    let cases = [
        (
            vec!["scene-profile", "streaming"],
            serde_json::json!({
                "message": "scene profile set: streaming",
                "hidden": 2,
                "warnings": [],
            }),
        ),
        (
            vec!["scene-profile", "--off"],
            serde_json::json!({ "message": "scene profile cleared" }),
        ),
        (
            vec!["scene-profile", "--delete", "night"],
            serde_json::json!({ "message": "scene profile deleted: night" }),
        ),
        // The listing is a structure rather than a sentence, and both spellings
        // of it must put that structure in `result` untouched.
        (
            vec!["scene-profiles"],
            serde_json::json!({
                "active": "streaming",
                "profiles": [{ "name": "streaming", "hidden": ["Utility BG"] }],
            }),
        ),
        (
            vec!["scene-profile"],
            serde_json::json!({
                "active": "streaming",
                "profiles": [{ "name": "streaming", "hidden": ["Utility BG"] }],
            }),
        ),
    ];

    for (command, response) in cases {
        let (_dir, config_path) = start_fake_ipc_server_with_config(response.clone());
        let mut args = vec!["--json", "--config", config_path.to_str().unwrap()];
        args.extend(command);

        let assert = obsctl()
            .args(args)
            .assert()
            .success()
            .stderr(predicates::str::is_empty());

        let parsed = parse_json_stdout(&assert.get_output().stdout);
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["result"], response);
        assert_eq!(parsed["error"], serde_json::Value::Null);
        assert_eq!(parsed["exit_code"], 0);
    }
}

/// Without `--json`, the listing answers with a structure rather than a
/// sentence, so it is printed field by field instead of as raw JSON.
///
/// Both spellings are checked because a user who guesses `scene-profile` and a
/// user who guesses `scene-profiles` are asking the same question and must get
/// the same answer.
#[test]
fn default_mode_prints_scene_profiles_as_fields() {
    let response = serde_json::json!({
        "active": "streaming",
        "profiles": [{ "name": "streaming", "hidden": ["Utility BG"] }],
    });

    for spelling in ["scene-profiles", "scene-profile"] {
        let (_dir, config_path) = start_fake_ipc_server_with_config(response.clone());

        obsctl()
            .args(["--config", config_path.to_str().unwrap(), spelling])
            .assert()
            .success()
            .stdout(
                contains("active: \"streaming\"")
                    .and(contains("profiles: "))
                    .and(predicates::str::is_match(r"^[^\{]").unwrap()),
            );
    }
}

/// An unknown scene profile is reported by the daemon as `CONFIG_INVALID`, and
/// that code already maps to exit 2 — no new public error code was introduced
/// for scene profiles.
#[test]
fn scene_profile_unknown_name_exits_2() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::error(
        "CONFIG_INVALID",
        "config invalid: scene profile not found: nope",
    ));

    let assert = obsctl()
        .args([
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "scene-profile",
            "nope",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::is_empty());

    let parsed = parse_json_stdout(&assert.get_output().stdout);
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["error"]["code"], "CONFIG_INVALID");
    assert_eq!(parsed["exit_code"], 2);
}

#[test]
fn json_flag_wraps_server_error_in_envelope() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::error(
        "OBS_UNAVAILABLE",
        "OBS is unavailable",
    ));

    let assert = obsctl()
        .args([
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "scene",
            "Main",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicates::str::is_empty());

    let parsed = parse_json_stdout(&assert.get_output().stdout);
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["result"], serde_json::Value::Null);
    assert_eq!(parsed["error"]["code"], "OBS_UNAVAILABLE");
    assert_eq!(parsed["error"]["message"], "OBS is unavailable");
    assert_eq!(parsed["exit_code"], 4);
}

#[test]
fn json_flag_wraps_reload_config_config_invalid_and_exits_2() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::error(
        "CONFIG_INVALID",
        "config invalid: invalid field",
    ));

    let assert = obsctl()
        .args([
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "reload-config",
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::is_empty());

    let parsed = parse_json_stdout(&assert.get_output().stdout);
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["result"], serde_json::Value::Null);
    assert_eq!(parsed["error"]["code"], "CONFIG_INVALID");
    assert_eq!(parsed["error"]["message"], "config invalid: invalid field");
    assert_eq!(parsed["exit_code"], 2);
}

#[test]
fn json_flag_redacts_unknown_daemon_error_message() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::error(
        "DAEMON_PRIVATE_CODE",
        "daemon failed with Password=hunter2 and token=abc.def",
    ));

    let assert = obsctl()
        .args([
            "--json",
            "--config",
            config_path.to_str().unwrap(),
            "status",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::is_empty());

    let parsed = parse_json_stdout(&assert.get_output().stdout);
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["result"], serde_json::Value::Null);
    assert_eq!(parsed["error"]["code"], "DAEMON_PRIVATE_CODE");
    assert_eq!(
        parsed["error"]["message"],
        "daemon failed with Password=[REDACTED] and token=[REDACTED]"
    );
    assert_eq!(parsed["exit_code"], 1);
}

#[test]
fn json_flag_wraps_local_server_unavailable_in_envelope() {
    let dir = TempDir::new().unwrap();
    let config = config_with_socket(&dir);

    let assert = obsctl()
        .args(["--json", "--config", config.to_str().unwrap(), "status"])
        .assert()
        .failure()
        .code(3)
        .stderr(predicates::str::is_empty());

    let parsed = parse_json_stdout(&assert.get_output().stdout);
    assert_eq!(parsed["ok"], false);
    assert_eq!(parsed["result"], serde_json::Value::Null);
    assert_eq!(parsed["error"]["code"], "SERVER_UNAVAILABLE");
    assert_eq!(parsed["exit_code"], 3);
    assert!(
        parsed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("obsctl server is not running")
    );
}

#[test]
fn default_mode_redacts_unknown_daemon_error_message() {
    let (_dir, config_path) = start_fake_ipc_server_with_reply(FakeIpcReply::error(
        "DAEMON_PRIVATE_CODE",
        "daemon failed for http://user:hunter2@example.test with Bearer abc.def",
    ));

    obsctl()
        .args(["--config", config_path.to_str().unwrap(), "status"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicates::str::is_empty())
        .stderr(
            contains("error [DAEMON_PRIVATE_CODE]: daemon failed for http://[REDACTED]@example.test with Bearer [REDACTED]")
                .and(predicates::str::contains("hunter2").not())
                .and(predicates::str::contains("abc.def").not())
                .and(predicates::str::contains("user").not()),
        );
}

#[test]
fn default_mode_emits_human_readable_for_obs_status() {
    let response = serde_json::json!({ "message": "obs is fine" });
    let (_dir, config_path) = start_fake_ipc_server_with_config(response);

    obsctl()
        .args(["--config", config_path.to_str().unwrap(), "obs-status"])
        .assert()
        .success()
        .stdout(contains("obs is fine").and(predicates::str::is_match(r"^[^\{]").unwrap()));
}
