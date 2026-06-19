use std::sync::Arc;
use std::time::Duration;

mod support {
    #[allow(dead_code)]
    pub mod fake_obs_server;
}

use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::{Mutex, mpsc, watch};

use obsctl_rs::{
    config::model::Config,
    ipc::{
        protocol::{
            CommandPayload, LogEvent, LogLevel, ServerMessage, TOPIC_LOGS,
            exit_code_for_public_error_code,
        },
        session::BroadcastHub,
        unix_client::IpcClient,
        unix_server::IpcServer,
    },
    obs::{
        client::ObsClient,
        connection::{ObsConnectionParams, connect},
        state::{ObsSnapshot, SceneState},
    },
    server::{
        client_registry::ClientRegistry, command_executor::CommandExecutor, state_store::StateStore,
    },
};
use support::fake_obs_server::{PreparedResponse, spawn_fake_obs};

async fn start_test_server_with_config(
    dir: &TempDir,
    config_path: &std::path::Path,
) -> (IpcClient, StateStore, watch::Sender<bool>) {
    let socket_path = dir.path().join("server_cfg.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new();
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    let state_clone = state.clone();
    let executor = CommandExecutor::new(
        state,
        obs_handle,
        config,
        Some(config_path.to_path_buf()),
        socket_path.clone(),
        registry,
        reconnect_tx,
        shutdown_tx.clone(),
        Arc::clone(&hub),
    );

    let server = IpcServer::bind(&socket_path, Arc::clone(&hub)).unwrap();
    tokio::spawn(executor.run(cmd_rx));
    tokio::spawn(server.run(cmd_tx, shutdown_rx));

    tokio::time::sleep(Duration::from_millis(20)).await;

    let client = IpcClient::connect(&socket_path).await.unwrap();
    (client, state_clone, shutdown_tx)
}

async fn start_test_server(dir: &TempDir) -> (IpcClient, watch::Sender<bool>) {
    let socket_path = dir.path().join("server.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new(); // no env var required in tests
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    let executor = CommandExecutor::new(
        state,
        obs_handle,
        config,
        None, // no config_path in tests
        socket_path.clone(),
        registry,
        reconnect_tx,
        shutdown_tx.clone(),
        Arc::clone(&hub),
    );

    let server = IpcServer::bind(&socket_path, Arc::clone(&hub)).unwrap();
    tokio::spawn(executor.run(cmd_rx));
    tokio::spawn(server.run(cmd_tx, shutdown_rx));

    tokio::time::sleep(Duration::from_millis(20)).await;

    let client = IpcClient::connect(&socket_path).await.unwrap();
    (client, shutdown_tx)
}

async fn start_test_server_with_obs_client(
    dir: &TempDir,
    cfg: Config,
    obs_client: ObsClient,
    snapshot: ObsSnapshot,
) -> (IpcClient, watch::Sender<bool>) {
    let socket_path = dir.path().join("server_obs.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    state.replace(snapshot).await;
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(Some(obs_client)));
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    let executor = CommandExecutor::new(
        state,
        obs_handle,
        config,
        None,
        socket_path.clone(),
        registry,
        reconnect_tx,
        shutdown_tx.clone(),
        Arc::clone(&hub),
    );

    let server = IpcServer::bind(&socket_path, Arc::clone(&hub)).unwrap();
    tokio::spawn(executor.run(cmd_rx));
    tokio::spawn(server.run(cmd_tx, shutdown_rx));

    tokio::time::sleep(Duration::from_millis(20)).await;

    let client = IpcClient::connect(&socket_path).await.unwrap();
    (client, shutdown_tx)
}

fn cmd(name: &str, args: Value) -> CommandPayload {
    CommandPayload {
        name: name.to_string(),
        args,
    }
}

fn extract_response(msg: ServerMessage) -> (bool, Option<Value>, Option<String>) {
    match msg {
        ServerMessage::Response {
            ok, result, error, ..
        } => (ok, result, error.map(|e| e.code)),
        other => panic!("expected Response, got {other:?}"),
    }
}

#[tokio::test]
async fn ping_returns_pong() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client.send_command(cmd("ping", Value::Null)).await.unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok);
    assert_eq!(result.unwrap()["message"], "pong");
}

#[tokio::test]
async fn get_server_status_returns_fields() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("get_server_status", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok);
    let data = result.unwrap();
    assert!(data.get("pid").is_some());
    assert!(data.get("uptime_seconds").is_some());
    assert_eq!(data["obs_connected"], false);
}

#[tokio::test]
async fn get_obs_status_when_disconnected() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("get_obs_status", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok);
    assert_eq!(result.unwrap()["connected"], false);
}

#[tokio::test]
async fn set_scene_returns_obs_unavailable_when_disconnected() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("set_scene", serde_json::json!({ "target": "Main" })))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok, "set_scene should fail without OBS");
    assert_eq!(code.as_deref(), Some("OBS_UNAVAILABLE"));
}

#[tokio::test]
async fn obs_command_timeout_returns_request_timeout_ipc_code_and_exit_4() {
    const TIMEOUT_MS: u64 = 75;
    let late_response_delay = Duration::from_millis(TIMEOUT_MS + 150);

    let fake_obs = spawn_fake_obs(false, None).await;
    fake_obs
        .set_response(
            "SetCurrentProgramScene",
            PreparedResponse::success(Value::Null).delayed(late_response_delay),
        )
        .await;

    let mut cfg = Config::default();
    cfg.connection.host = "127.0.0.1".to_string();
    cfg.connection.port = fake_obs.addr.port();
    cfg.connection.password_env = String::new();
    cfg.connection.request_timeout_ms = TIMEOUT_MS;
    let params = ObsConnectionParams::from_config(&cfg.connection);
    let (event_tx, _event_rx) = mpsc::channel(8);
    let (obs_client, _, _) = connect(&params, event_tx).await.unwrap();

    let snapshot = ObsSnapshot {
        connected: true,
        current_scene: Some("Main".to_string()),
        scenes: vec![SceneState {
            name: "Main".to_string(),
            active: true,
            ..SceneState::default()
        }],
        ..ObsSnapshot::default()
    };
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) =
        start_test_server_with_obs_client(&dir, cfg, obs_client, snapshot).await;

    let resp = client
        .send_command(cmd("set_scene", serde_json::json!({ "target": "Main" })))
        .await
        .unwrap();
    let (code, message) = match resp {
        ServerMessage::Response {
            ok: false,
            error: Some(error),
            ..
        } => (error.code, error.message),
        other => panic!("expected timeout error response, got {other:?}"),
    };
    assert_eq!(code, "REQUEST_TIMEOUT");
    assert_eq!(exit_code_for_public_error_code(&code), 4);
    assert_eq!(message, "request timed out");

    tokio::time::sleep(late_response_delay + Duration::from_millis(50)).await;
    fake_obs
        .set_response(
            "SetCurrentProgramScene",
            PreparedResponse::success(Value::Null),
        )
        .await;

    let resp = client
        .send_command(cmd("set_scene", serde_json::json!({ "target": "Main" })))
        .await
        .unwrap();
    let (ok, result, code) = extract_response(resp);
    assert!(ok, "subsequent command should succeed: {code:?}");
    assert_eq!(result.unwrap()["message"], "scene set: Main");

    fake_obs.shutdown();
}

#[tokio::test]
async fn unknown_command_returns_error() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("nonexistent_cmd", Value::Null))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok);
    assert_eq!(code.as_deref(), Some("COMMAND_PARSE_ERROR"));
}

#[tokio::test]
async fn validate_config_succeeds_with_default_config() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("validate_config", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "validate_config should succeed with defaults");
    assert_eq!(result.unwrap()["valid"], true);
}

#[tokio::test]
async fn get_snapshot_returns_state_fields() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("get_snapshot", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok);
    let data = result.unwrap();
    assert!(data.get("connected").is_some());
    assert!(data.get("scenes").is_some());
    assert!(data.get("audio_inputs").is_some());
}

#[tokio::test]
async fn mute_target_missing_returns_obs_unavailable_without_obs() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown) = start_test_server(&dir).await;

    let resp = client
        .send_command(cmd("mute", serde_json::json!({ "target": "Mic" })))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok);
    assert_eq!(code.as_deref(), Some("OBS_UNAVAILABLE"));
}

#[tokio::test]
async fn socket_file_exists_while_server_runs() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("server.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new();
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    let executor = CommandExecutor::new(
        state,
        obs_handle,
        config,
        None,
        socket_path.clone(),
        registry,
        reconnect_tx,
        shutdown_tx.clone(),
        Arc::clone(&hub),
    );

    let server = IpcServer::bind(&socket_path, Arc::clone(&hub)).unwrap();
    tokio::spawn(executor.run(cmd_rx));
    tokio::spawn(server.run(cmd_tx, shutdown_rx));

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        socket_path.exists(),
        "socket file should exist while server is running"
    );

    // Signal shutdown
    let _ = shutdown_tx.send(true);
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn server_handles_multiple_sequential_clients() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("server.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new();
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    let executor = CommandExecutor::new(
        state,
        obs_handle,
        config,
        None,
        socket_path.clone(),
        registry,
        reconnect_tx,
        shutdown_tx.clone(),
        Arc::clone(&hub),
    );

    let server = IpcServer::bind(&socket_path, Arc::clone(&hub)).unwrap();
    tokio::spawn(executor.run(cmd_rx));
    tokio::spawn(server.run(cmd_tx, shutdown_rx));

    tokio::time::sleep(Duration::from_millis(20)).await;

    // Connect multiple clients sequentially and verify each gets a valid response.
    for i in 0..3 {
        let mut client = IpcClient::connect(&socket_path).await.unwrap();
        let resp = client.send_command(cmd("ping", Value::Null)).await.unwrap();
        let (ok, result, _) = extract_response(resp);
        assert!(ok, "client {i} ping failed");
        assert_eq!(result.unwrap()["message"], "pong");
    }
}

const VALID_TEST_CONFIG_YAML: &str = r#"version: 1
server:
  socket_path: ~
  pid_file: ~
  allow_remote_shutdown: false
  start_embedded_if_missing: true
connection:
  host: "127.0.0.1"
  port: 4455
  password_env: ""
  connect_timeout_ms: 3000
  request_timeout_ms: 2500
reconnect:
  enabled: true
  endless: true
  initial_delay_ms: 500
  max_delay_ms: 10000
  multiplier: 1.8
  jitter_ms: 250
ui:
  refresh_interval_ms: 250
  command_palette_prefix: "/"
  show_icons: true
  theme: "default"
scenes: []
audio:
  inputs: []
keymap:
  quit: ["q", "ctrl+c"]
  command_palette: ["/", ":"]
  reload_config: ["r"]
  dump_config: ["D"]
"#;

#[tokio::test]
async fn reload_config_updates_scene_aliases_in_snapshot() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.yml");
    std::fs::write(&config_path, VALID_TEST_CONFIG_YAML).unwrap();

    let (mut client, state, _shutdown) = start_test_server_with_config(&dir, &config_path).await;

    // Pre-seed the state as if OBS had reported a scene list.
    state
        .replace(ObsSnapshot {
            scenes: vec![SceneState {
                name: "Main Scene".to_string(),
                ..SceneState::default()
            }],
            ..ObsSnapshot::default()
        })
        .await;

    // Write an updated config that assigns an alias to the existing scene.
    let updated_yaml = VALID_TEST_CONFIG_YAML.replace(
        "scenes: []",
        "scenes:\n  - name: \"Main Scene\"\n    alias: \"main\"",
    );
    std::fs::write(&config_path, updated_yaml).unwrap();

    let resp = client
        .send_command(cmd("reload_config", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "reload_config should succeed with valid config");
    assert_eq!(result.unwrap()["message"], "config reloaded");

    // The snapshot should now reflect the alias from the reloaded config.
    let resp = client
        .send_command(cmd("get_snapshot", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok);
    let data = result.unwrap();
    let scenes = data["scenes"].as_array().expect("scenes array");
    assert_eq!(scenes.len(), 1, "scene count should be unchanged");
    assert_eq!(scenes[0]["name"], "Main Scene");
    assert_eq!(
        scenes[0]["alias"], "main",
        "alias should reflect reloaded config"
    );
}

#[tokio::test]
async fn reload_config_publishes_typed_log_event() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.yml");
    let socket_path = dir.path().join("server_cfg.sock");
    std::fs::write(&config_path, VALID_TEST_CONFIG_YAML).unwrap();

    let (mut command_client, _state, _shutdown) =
        start_test_server_with_config(&dir, &config_path).await;
    let mut logs_client = IpcClient::connect(&socket_path).await.unwrap();
    logs_client.subscribe(&[TOPIC_LOGS]).await.unwrap();

    let resp = command_client
        .send_command(cmd("reload_config", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "reload_config should succeed with valid config");
    assert_eq!(result.unwrap()["message"], "config reloaded");

    let event = logs_client.next_event().await.unwrap();
    match event {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, TOPIC_LOGS);
            let log_event: LogEvent = serde_json::from_value(data).unwrap();
            assert_eq!(log_event.level, LogLevel::Info);
            assert_eq!(log_event.message, "Config reloaded");
            assert_eq!(
                log_event.target.as_deref(),
                Some("obsctl_rs::server::command_executor")
            );
        }
        other => panic!("expected log event, got {other:?}"),
    }
}

#[tokio::test]
async fn reload_config_returns_config_invalid_for_bad_file() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.yml");
    std::fs::write(&config_path, VALID_TEST_CONFIG_YAML).unwrap();

    let (mut client, _state, _shutdown) = start_test_server_with_config(&dir, &config_path).await;

    // Overwrite with a config that will fail schema validation.
    std::fs::write(&config_path, "version: 99\n").unwrap();

    let resp = client
        .send_command(cmd("reload_config", Value::Null))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok, "reload_config should fail for invalid config");
    assert_eq!(
        code.as_deref(),
        Some("CONFIG_INVALID"),
        "error code should be CONFIG_INVALID"
    );

    // Server should remain usable with the previous config still active.
    let resp = client.send_command(cmd("ping", Value::Null)).await.unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "server should still respond after failed reload");
    assert_eq!(result.unwrap()["message"], "pong");
}
