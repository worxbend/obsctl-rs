use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::{Mutex, mpsc, watch};

use obsctl_rs::{
    config::model::Config,
    ipc::{
        protocol::{CommandPayload, ServerMessage},
        session::BroadcastHub,
        unix_client::IpcClient,
        unix_server::IpcServer,
    },
    obs::client::ObsClient,
    server::{
        client_registry::ClientRegistry, command_executor::CommandExecutor, state_store::StateStore,
    },
};

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
        Arc::clone(&hub),
        config,
        None, // no config_path in tests
        socket_path.clone(),
        registry,
        reconnect_tx,
        shutdown_tx.clone(),
    );

    let server = IpcServer::bind(&socket_path, hub).unwrap();
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
