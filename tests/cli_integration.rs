// CLI integration tests.

use std::sync::Arc;

use assert_cmd::Command;
use obsctl_rs::ipc::{
    protocol::{ErrorPayload, ServerMessage},
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
    Error { code: String, message: String },
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

    fn into_response(self, id: String) -> ServerMessage {
        match self {
            Self::Success(result) => ServerMessage::Response {
                id,
                ok: true,
                result: Some(result),
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
                let msg = reply_bg.clone().into_response(dispatch.id.clone());
                let _ = dispatch.reply.send(msg);
            }
        });
    });

    ready_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("fake IPC server should bind");

    (dir, config_path)
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
