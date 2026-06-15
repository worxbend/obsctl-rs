// CLI integration tests.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
use tempfile::TempDir;

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
    std::fs::write(&path, "version: 1\nconnection: !!!bad").unwrap();

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

fn nonexistent_socket() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/obsctl-nonexistent-test.sock")
}

fn config_with_socket(dir: &TempDir) -> std::path::PathBuf {
    let sock = nonexistent_socket();
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
