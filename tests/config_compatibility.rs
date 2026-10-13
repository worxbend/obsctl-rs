use obsctl_rs::config::{dump, loader, model::Config, writer};
use std::path::Path;

fn private_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    directory
}

const SHARED_CONFIG: &str = include_str!("fixtures/config/shared.yml");

fn load_yaml(directory: &Path, yaml: &str) -> Config {
    let path = directory.join("config.yml");
    std::fs::write(&path, yaml).unwrap();
    loader::load(&path).unwrap()
}

#[test]
fn crystal_settings_survive_dump_and_save() {
    let directory = private_tempdir();
    let config = load_yaml(directory.path(), SHARED_CONFIG);
    assert_eq!(config.server.pid_file.as_deref(), Some("/tmp/obsctl.pid"));
    assert!(!config.server.start_embedded_if_missing);
    assert_eq!(config.audio.inputs[0].kind, "wasapi_input_capture");
    assert!(!config.ui.mouse);

    let merged = dump::merge(
        &config,
        &dump::ObsResources {
            scenes: vec!["Main Camera".into()],
            inputs: vec!["Mic/Aux".into(), "Desktop Audio".into()],
        },
    )
    .unwrap();
    assert_eq!(merged.audio.inputs[0].kind, "wasapi_input_capture");
    assert_eq!(merged.audio.inputs[1].kind, "input");
    let saved = directory.path().join("saved.yml");
    writer::write_atomic(&merged, &saved).unwrap();
    let reloaded = loader::load(&saved).unwrap();
    assert_eq!(
        serde_json::to_value(&reloaded).unwrap(),
        serde_json::to_value(&merged).unwrap()
    );
}

#[test]
fn missing_version_and_null_password_env_match_crystal_defaults() {
    let directory = private_tempdir();
    let defaults = load_yaml(directory.path(), "{}\n");
    assert_eq!(defaults.version, 1);
    assert_eq!(defaults.ui.command_palette_prefix, "/");
    assert_eq!(defaults.connection.password_env, "OBS_WEBSOCKET_PASSWORD");
    for value in ["", "null", "~", "\"\""] {
        let config = load_yaml(
            directory.path(),
            &format!("connection:\n  password_env: {value}\n"),
        );
        assert!(config.connection.password_env.is_empty());
    }
}

#[test]
fn canonical_reconnect_wins_over_legacy_even_with_explicit_defaults() {
    let directory = private_tempdir();
    let legacy = "connection:\n  password_env: \"\"\n  reconnect:\n    enabled: false\n    initial_delay_ms: 25\n";
    assert!(!load_yaml(directory.path(), legacy).reconnect.enabled);
    for canonical in ["{}", "{enabled: true, initial_delay_ms: 500}"] {
        let config = load_yaml(
            directory.path(),
            &format!("{legacy}reconnect: {canonical}\n"),
        );
        assert!(config.reconnect.enabled);
        assert_eq!(config.reconnect.initial_delay_ms, 500);
        assert!(config.connection.reconnect.is_none());
        let saved = directory.path().join("canonical.yml");
        writer::write_atomic(&config, &saved).unwrap();
        let serialized = std::fs::read_to_string(saved).unwrap();
        assert_eq!(serialized.matches("reconnect:").count(), 1);
    }
}

#[test]
#[ignore = "requires Crystal and the sibling ../obsctl checkout"]
fn config_round_trips_through_crystal_without_changing_values() {
    let directory = private_tempdir();
    let rust_saved = directory.path().join("rust.yml");
    let crystal_saved = directory.path().join("crystal.yml");
    for yaml in [
        SHARED_CONFIG,
        "{}",
        "connection:\n  password_env: null\n  password: fixture-password\n",
        "connection:\n  password_env: OBSCTL_COMPAT_PASSWORD\n  password: fixture-fallback\n  reconnect:\n    enabled: false\nreconnect: {}\n",
    ] {
        let config = load_yaml(directory.path(), yaml);
        writer::write_atomic(&config, &rust_saved).unwrap();

        let output = std::process::Command::new("crystal")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(["run", "tests/support/config_roundtrip.cr", "--"])
            .arg(&rust_saved)
            .arg(&crystal_saved)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reloaded = loader::load(&crystal_saved).unwrap();
        assert_eq!(
            serde_json::to_value(&reloaded).unwrap(),
            serde_json::to_value(&config).unwrap()
        );
    }
}
