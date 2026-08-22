use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
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
            ClientMessage, CommandPayload, LogEvent, LogLevel, ServerMessage, TOPIC_EVENTS,
            TOPIC_LOGS, TOPIC_STATE, Topic, exit_code_for_public_error_code,
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
        client_registry::ClientRegistry,
        command_executor::{CommandExecutor, CommandExecutorConfig},
        command_lanes::ExecutorLanes,
        obs_supervisor::{ObsSupervisor, ObsSupervisorConfig},
        state_store::StateStore,
    },
};
use support::fake_obs_server::{PreparedResponse, spawn_fake_obs, spawn_silent_obs};

#[test]
fn ipc_protocol_cli_and_tui_do_not_import_obs_client() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert_no_obs_client_import(&root.join("src/ipc/protocol.rs"));
    assert_no_obs_client_imports_in_dir(&root.join("src/cli"));
    assert_no_obs_client_imports_in_dir(&root.join("src/tui"));
}

fn assert_no_obs_client_imports_in_dir(dir: &Path) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            assert_no_obs_client_imports_in_dir(&path);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            assert_no_obs_client_import(&path);
        }
    }
}

fn assert_no_obs_client_import(path: &Path) {
    let source = fs::read_to_string(path).unwrap();
    assert!(
        !source.contains("obs::client"),
        "{} must not import obs::client",
        path.display()
    );
}

async fn start_test_server_with_config(
    dir: &TempDir,
    config_path: &std::path::Path,
) -> (IpcClient, StateStore, watch::Sender<bool>) {
    let socket_path = dir.path().join("server_cfg.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let reconnecting = Arc::new(AtomicBool::new(false));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new();
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let state_clone = state.clone();
    let executor = CommandExecutor::new(CommandExecutorConfig {
        state,
        obs: obs_handle,
        config,
        config_path: Some(config_path.to_path_buf()),
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx,
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });

    let server = IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry).unwrap();
    tokio::spawn(server.run(Arc::new(ExecutorLanes::new(executor)), shutdown_rx));

    tokio::time::sleep(Duration::from_millis(20)).await;

    let client = IpcClient::connect(&socket_path).await.unwrap();
    (client, state_clone, shutdown_tx)
}

async fn start_test_server(dir: &TempDir) -> (IpcClient, watch::Sender<bool>) {
    let socket_path = dir.path().join("server.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let reconnecting = Arc::new(AtomicBool::new(false));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new(); // no env var required in tests
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let executor = CommandExecutor::new(CommandExecutorConfig {
        state,
        obs: obs_handle,
        config,
        config_path: None,
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx,
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });

    let server = IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry).unwrap();
    tokio::spawn(server.run(Arc::new(ExecutorLanes::new(executor)), shutdown_rx));

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
    let (socket_path, shutdown_tx) =
        spawn_server_with_obs_client(dir, cfg, obs_client, snapshot).await;
    let client = IpcClient::connect(&socket_path).await.unwrap();
    (client, shutdown_tx)
}

/// The same daemon as [`start_test_server_with_obs_client`], handed back by its
/// socket path instead of by one connected client — which is what a test needs
/// when it wants two clients talking to the daemon at once.
async fn spawn_server_with_obs_client(
    dir: &TempDir,
    cfg: Config,
    obs_client: ObsClient,
    snapshot: ObsSnapshot,
) -> (PathBuf, watch::Sender<bool>) {
    let socket_path = dir.path().join("server_obs.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    state.seed_for_tests(snapshot).await;
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(Some(obs_client)));
    let reconnecting = Arc::new(AtomicBool::new(false));
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let executor = CommandExecutor::new(CommandExecutorConfig {
        state,
        obs: obs_handle,
        config,
        config_path: None,
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx,
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });

    let server = IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry).unwrap();
    tokio::spawn(server.run(Arc::new(ExecutorLanes::new(executor)), shutdown_rx));

    wait_for_ipc_available(&socket_path).await;

    (socket_path, shutdown_tx)
}

async fn start_test_server_with_obs_supervisor(
    dir: &TempDir,
    cfg: Config,
) -> (PathBuf, StateStore, Arc<BroadcastHub>, watch::Sender<bool>) {
    let socket_path = dir.path().join("server_obs_supervisor.sock");

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let reconnecting = Arc::new(AtomicBool::new(false));
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let executor = CommandExecutor::new(CommandExecutorConfig {
        state: state.clone(),
        obs: Arc::clone(&obs_handle),
        config: Arc::clone(&config),
        config_path: None,
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx,
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });
    let supervisor = ObsSupervisor::new(ObsSupervisorConfig {
        config: Arc::clone(&config),
        state: state.clone(),
        obs_handle,
        reconnecting: Arc::clone(&reconnecting),
        reconnect_rx,
        shutdown: shutdown_rx.clone(),
        hub: Arc::clone(&hub),
    });

    let server = IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry).unwrap();
    tokio::spawn(server.run(Arc::new(ExecutorLanes::new(executor)), shutdown_rx));
    tokio::spawn(supervisor.run());

    wait_for_ipc_available(&socket_path).await;

    (socket_path, state, hub, shutdown_tx)
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

async fn wait_for_obs_connected(state: &StateStore) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if state.read().await.connected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("OBS supervisor did not connect to fake OBS");
}

async fn wait_for_ipc_available(socket_path: &Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if IpcClient::connect(socket_path).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("IPC server did not become available");
}

async fn next_event_with_timeout(client: &mut IpcClient) -> ServerMessage {
    tokio::time::timeout(Duration::from_millis(500), client.next_event())
        .await
        .expect("timed out waiting for IPC event")
        .expect("failed to read IPC event")
}

async fn expect_obs_event(client: &mut IpcClient, expected: Value) {
    match next_event_with_timeout(client).await {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, Topic::Events);
            assert_eq!(data, expected);
        }
        other => panic!("expected OBS event, got {other:?}"),
    }
}

async fn next_state_event_matching<F>(
    client: &mut IpcClient,
    description: &str,
    matches: F,
) -> Value
where
    F: Fn(&Value) -> bool,
{
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match client
                .next_event()
                .await
                .expect("failed to read state event")
            {
                ServerMessage::Event { topic, data } => {
                    assert_eq!(topic, Topic::State);
                    assert_ne!(
                        data.get("type").and_then(Value::as_str),
                        Some("CurrentProgramSceneChanged")
                    );
                    assert_ne!(
                        data.get("type").and_then(Value::as_str),
                        Some("InputMuteStateChanged")
                    );
                    if matches(&data) {
                        return data;
                    }
                }
                other => panic!("expected state event, got {other:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for state event with {description}"))
}

async fn drain_logs_until_marker(client: &mut IpcClient, marker: &str) -> Vec<Value> {
    let mut logs = Vec::new();
    loop {
        match next_event_with_timeout(client).await {
            ServerMessage::Event { topic, data } => {
                assert_eq!(topic, Topic::Logs);
                if data.get("message").and_then(Value::as_str) == Some(marker) {
                    return logs;
                }
                logs.push(data);
            }
            other => panic!("expected log event, got {other:?}"),
        }
    }
}

fn publish_log_marker(hub: &BroadcastHub, marker: &str) {
    hub.publish_log(LogEvent::new(LogLevel::Info, marker).with_target("tests::server_integration"));
}

fn assert_logs_do_not_receive_obs_payloads(logs: &[Value], forbidden_types: &[&str]) {
    for data in logs {
        let payload_type = data.get("type").and_then(Value::as_str);
        assert!(
            !forbidden_types.contains(&payload_type.unwrap_or_default()),
            "logs-only subscriber received OBS event payload: {data}"
        );
    }
}

fn assert_logs_do_not_contain_text(logs: &[Value], forbidden: &str) {
    for data in logs {
        assert!(
            !data.to_string().contains(forbidden),
            "logs subscriber received forbidden payload text `{forbidden}`: {data}"
        );
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
    assert!(data["client_count"].as_u64().unwrap() >= 1);
    assert_eq!(data["obs_connected"], false);
    assert_eq!(data["reconnecting"], false);
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
    let params = ObsConnectionParams::from_config(&cfg.connection).unwrap();
    let (event_tx, _event_rx) = mpsc::channel(8);
    let (obs_client, _, _, _disconnect) = connect(&params, event_tx).await.unwrap();

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

/// A scene change that arrives while a full refresh is in flight must not be
/// left overwritten by that refresh.
///
/// A full refresh is a dozen-plus obs-websocket round-trips, and it builds its
/// result from replies gathered across all of them. An event applied part-way
/// through is therefore not represented in it, and publishing it reverts that
/// event. Holding the store's write lock makes the swap atomic; it does not
/// make the data current, which is what two comments in the daemon used to
/// claim.
///
/// The delay on `GetInputList` puts a window in the middle of the refresh —
/// after the current scene has been read, before the refresh completes — and
/// the event is emitted into it.
#[tokio::test]
async fn a_scene_change_during_a_refresh_is_not_left_reverted() {
    let fake_obs = spawn_fake_obs(false, None).await;

    let mut cfg = Config::default();
    cfg.connection.host = "127.0.0.1".to_string();
    cfg.connection.port = fake_obs.addr.port();
    cfg.connection.password_env = String::new();
    cfg.connection.request_timeout_ms = 4000;

    let dir = TempDir::new().unwrap();
    let (_socket_path, state, _hub, shutdown) =
        start_test_server_with_obs_supervisor(&dir, cfg).await;
    wait_for_obs_connected(&state).await;

    // Every refresh now stalls here, part-way through, after the scene list
    // and current scene have been read.
    fake_obs
        .set_response(
            "GetInputList",
            PreparedResponse::success(serde_json::json!({ "inputs": [] }))
                .delayed(std::time::Duration::from_millis(400)),
        )
        .await;
    fake_obs
        .set_response(
            "GetSceneList",
            PreparedResponse::success(serde_json::json!({
                "currentProgramSceneName": "Main",
                "scenes": [
                    { "sceneName": "Main", "sceneIndex": 0 },
                    { "sceneName": "Live", "sceneIndex": 1 }
                ]
            })),
        )
        .await;

    // Start a full refresh. `SceneCreated` is one of the events the supervisor
    // answers with a complete re-fetch.
    fake_obs.emit_event("SceneCreated", serde_json::json!({ "sceneName": "Live" }));

    // Let it get past the scene reads and into the stalled `GetInputList`.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // OBS switches scene mid-refresh. The in-flight fetch already read "Main"
    // and knows nothing about this.
    fake_obs
        .set_response(
            "GetCurrentProgramScene",
            PreparedResponse::success(serde_json::json!({
                "currentProgramSceneName": "Live"
            })),
        )
        .await;
    fake_obs.emit_event(
        "CurrentProgramSceneChanged",
        serde_json::json!({ "sceneName": "Live" }),
    );

    // The stale refresh lands and reverts the scene to "Main" — unavoidable,
    // its data predates the event. What must not happen is that it stays that
    // way: the daemon should notice it was overtaken and fetch again.
    //
    // Both halves of the condition matter. "Live" as the current scene alone
    // is also true for a moment right after the event and before the stale
    // refresh overwrites it, so it would pass without the fix; "Live" present
    // in the scene list proves a refresh has since completed. Only the
    // re-fetch produces both at once.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let snap = state.read().await;
        let refreshed = snap.scenes.iter().any(|scene| scene.name == "Live");
        if refreshed && snap.current_scene.as_deref() == Some("Live") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "refresh overtaken by a scene change was never corrected: \
             current_scene={:?} scenes={:?}",
            snap.current_scene,
            snap.scenes.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        drop(snap);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    shutdown.send(true).unwrap();
    fake_obs.shutdown();
}

#[tokio::test]
async fn obs_supervisor_publishes_obs_events_only_to_events_subscribers() {
    let fake_obs = spawn_fake_obs(false, None).await;

    let mut cfg = Config::default();
    cfg.connection.host = "127.0.0.1".to_string();
    cfg.connection.port = fake_obs.addr.port();
    cfg.connection.password_env = String::new();
    cfg.connection.request_timeout_ms = 500;

    let dir = TempDir::new().unwrap();
    let (socket_path, state, hub, shutdown) =
        start_test_server_with_obs_supervisor(&dir, cfg).await;
    wait_for_obs_connected(&state).await;

    let mut events_client = IpcClient::connect(&socket_path).await.unwrap();
    events_client.subscribe(&[TOPIC_EVENTS]).await.unwrap();
    let mut state_client = IpcClient::connect(&socket_path).await.unwrap();
    state_client.subscribe(&[TOPIC_STATE]).await.unwrap();
    let mut logs_client = IpcClient::connect(&socket_path).await.unwrap();
    logs_client.subscribe(&[TOPIC_LOGS]).await.unwrap();

    fake_obs.emit_event(
        "CurrentProgramSceneChanged",
        serde_json::json!({ "sceneName": "BRB" }),
    );

    expect_obs_event(
        &mut events_client,
        serde_json::json!({
            "type": "CurrentProgramSceneChanged",
            "scene_name": "BRB"
        }),
    )
    .await;

    let scene_state =
        next_state_event_matching(&mut state_client, "current_scene updated to BRB", |data| {
            data["current_scene"] == "BRB"
        })
        .await;
    assert_eq!(scene_state["current_scene"], "BRB");

    fake_obs.emit_event(
        "InputMuteStateChanged",
        serde_json::json!({ "inputName": "Mic", "inputMuted": true }),
    );

    expect_obs_event(
        &mut events_client,
        serde_json::json!({
            "type": "InputMuteStateChanged",
            "input_name": "Mic",
            "muted": true
        }),
    )
    .await;

    let muted_state = next_state_event_matching(&mut state_client, "Mic input muted", |data| {
        data["audio_inputs"].as_array().is_some_and(|inputs| {
            inputs
                .iter()
                .any(|input| input["name"] == "Mic" && input["muted"] == true)
        })
    })
    .await;
    let mic = muted_state["audio_inputs"]
        .as_array()
        .expect("audio_inputs array")
        .iter()
        .find(|input| input["name"] == "Mic")
        .expect("Mic input");
    assert_eq!(mic["muted"], true);

    fake_obs
        .set_response(
            "GetSceneList",
            PreparedResponse::success(serde_json::json!({
                "currentProgramSceneName": "BRB",
                "scenes": [
                    { "sceneName": "Main", "sceneIndex": 0 },
                    { "sceneName": "BRB", "sceneIndex": 1 },
                    { "sceneName": "Interview", "sceneIndex": 2 }
                ]
            })),
        )
        .await;
    fake_obs
        .set_response(
            "GetCurrentProgramScene",
            PreparedResponse::success(serde_json::json!({
                "currentProgramSceneName": "BRB"
            })),
        )
        .await;
    fake_obs.emit_event(
        "SceneCreated",
        serde_json::json!({ "sceneName": "Interview" }),
    );

    expect_obs_event(
        &mut events_client,
        serde_json::json!({
            "type": "SceneListChanged"
        }),
    )
    .await;

    let scene_list_state =
        next_state_event_matching(&mut state_client, "scene list mutation broadcast", |data| {
            data["connected"] == true
                && data["current_scene"] == "BRB"
                && data["scenes"]
                    .as_array()
                    .is_some_and(|scenes| scenes.iter().any(|scene| scene["name"] == "Interview"))
        })
        .await;
    assert_eq!(scene_list_state["current_scene"], "BRB");
    assert!(
        scene_list_state["scenes"]
            .as_array()
            .expect("scenes array")
            .iter()
            .any(|scene| scene["name"] == "Interview")
    );

    fake_obs.emit_event(
        "InputCreated",
        serde_json::json!({ "inputName": "Browser Audio" }),
    );

    expect_obs_event(
        &mut events_client,
        serde_json::json!({
            "type": "InputCreated",
            "input_name": "Browser Audio"
        }),
    )
    .await;

    let input_created_state =
        next_state_event_matching(&mut state_client, "Browser Audio input created", |data| {
            data["audio_inputs"]
                .as_array()
                .is_some_and(|inputs| inputs.iter().any(|input| input["name"] == "Browser Audio"))
        })
        .await;
    assert!(
        input_created_state["audio_inputs"]
            .as_array()
            .expect("audio_inputs array")
            .iter()
            .any(|input| input["name"] == "Browser Audio")
    );

    fake_obs.emit_event(
        "InputVolumeChanged",
        serde_json::json!({
            "inputName": "Browser Audio",
            "inputVolumeMul": 0.42,
            "inputVolumeDb": -7.5
        }),
    );

    expect_obs_event(
        &mut events_client,
        serde_json::json!({
            "type": "InputVolumeChanged",
            "input_name": "Browser Audio",
            "volume_mul": 0.42,
            "volume_db": -7.5
        }),
    )
    .await;

    let volume_state =
        next_state_event_matching(&mut state_client, "Browser Audio volume updated", |data| {
            data["audio_inputs"].as_array().is_some_and(|inputs| {
                inputs.iter().any(|input| {
                    input["name"] == "Browser Audio"
                        && input["volume_mul"] == 0.42
                        && input["volume_db"] == -7.5
                        && input["volume_percent"] == 65
                })
            })
        })
        .await;
    let browser = volume_state["audio_inputs"]
        .as_array()
        .expect("audio_inputs array")
        .iter()
        .find(|input| input["name"] == "Browser Audio")
        .expect("Browser Audio input");
    assert_eq!(browser["volume_mul"], 0.42);
    assert_eq!(browser["volume_db"], -7.5);
    assert_eq!(browser["volume_percent"], 65);

    fake_obs.emit_event(
        "InputRemoved",
        serde_json::json!({ "inputName": "Browser Audio" }),
    );

    expect_obs_event(
        &mut events_client,
        serde_json::json!({
            "type": "InputRemoved",
            "input_name": "Browser Audio"
        }),
    )
    .await;

    let input_removed_state =
        next_state_event_matching(&mut state_client, "Browser Audio input removed", |data| {
            data["audio_inputs"]
                .as_array()
                .is_some_and(|inputs| inputs.iter().all(|input| input["name"] != "Browser Audio"))
        })
        .await;
    assert!(
        input_removed_state["audio_inputs"]
            .as_array()
            .expect("audio_inputs array")
            .iter()
            .all(|input| input["name"] != "Browser Audio")
    );

    let known_events_log_marker = "obs-event-routing-known-events-complete";
    publish_log_marker(&hub, known_events_log_marker);
    let logs_before_marker =
        drain_logs_until_marker(&mut logs_client, known_events_log_marker).await;
    assert_logs_do_not_receive_obs_payloads(
        &logs_before_marker,
        &[
            "CurrentProgramSceneChanged",
            "InputMuteStateChanged",
            "SceneListChanged",
            "InputCreated",
            "InputVolumeChanged",
            "InputRemoved",
        ],
    );

    let state_before_unknown = serde_json::to_value(state.read().await).unwrap();
    let mut unknown_events_client = IpcClient::connect(&socket_path).await.unwrap();
    unknown_events_client
        .subscribe(&[TOPIC_EVENTS])
        .await
        .unwrap();
    let mut unknown_logs_client = IpcClient::connect(&socket_path).await.unwrap();
    unknown_logs_client.subscribe(&[TOPIC_LOGS]).await.unwrap();

    fake_obs.emit_event(
        "VendorSpecificEvent",
        serde_json::json!({
            "sceneName": "SHOULD_NOT_APPEAR",
            "inputName": "SHOULD_NOT_APPEAR",
            "inputMuted": false
        }),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        serde_json::to_value(state.read().await).unwrap(),
        state_before_unknown
    );

    fake_obs.emit_event(
        "InputMuteStateChanged",
        serde_json::json!({ "inputName": "Mic", "inputMuted": true }),
    );

    expect_obs_event(
        &mut unknown_events_client,
        serde_json::json!({
            "type": "InputMuteStateChanged",
            "input_name": "Mic",
            "muted": true
        }),
    )
    .await;

    assert_eq!(
        state
            .read()
            .await
            .audio_inputs
            .iter()
            .find(|input| input.name == "Mic")
            .and_then(|input| input.muted),
        Some(true)
    );
    let unknown_event_log_marker = "obs-event-routing-unknown-event-complete";
    publish_log_marker(&hub, unknown_event_log_marker);
    let unknown_logs_before_marker =
        drain_logs_until_marker(&mut unknown_logs_client, unknown_event_log_marker).await;
    assert_logs_do_not_contain_text(&unknown_logs_before_marker, "VendorSpecificEvent");

    let _ = shutdown.send(true);
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
    let reconnecting = Arc::new(AtomicBool::new(false));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new();
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let executor = CommandExecutor::new(CommandExecutorConfig {
        state,
        obs: obs_handle,
        config,
        config_path: None,
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx,
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });

    let server = IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry).unwrap();
    tokio::spawn(server.run(Arc::new(ExecutorLanes::new(executor)), shutdown_rx));

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
    let reconnecting = Arc::new(AtomicBool::new(false));
    let mut cfg = Config::default();
    cfg.connection.password_env = String::new();
    let config = Arc::new(Mutex::new(cfg));
    let registry = ClientRegistry::new();
    let (reconnect_tx, _reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let executor = CommandExecutor::new(CommandExecutorConfig {
        state,
        obs: obs_handle,
        config,
        config_path: None,
        socket_path: socket_path.clone(),
        registry: registry.clone(),
        reconnecting: Arc::clone(&reconnecting),
        reconnect_tx,
        shutdown_tx: shutdown_tx.clone(),
        hub: Arc::clone(&hub),
    });

    let server = IpcServer::bind_with_registry(&socket_path, Arc::clone(&hub), registry).unwrap();
    tokio::spawn(server.run(Arc::new(ExecutorLanes::new(executor)), shutdown_rx));

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
        .seed_for_tests(ObsSnapshot {
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
            assert_eq!(topic, Topic::Logs);
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

async fn start_obs_connected_server(
    dir: &TempDir,
) -> (
    IpcClient,
    watch::Sender<bool>,
    support::fake_obs_server::FakeObsHandle,
) {
    let fake_obs = spawn_fake_obs(false, None).await;

    let mut cfg = Config::default();
    cfg.connection.host = "127.0.0.1".to_string();
    cfg.connection.port = fake_obs.addr.port();
    cfg.connection.password_env = String::new();
    cfg.connection.request_timeout_ms = 500;
    let params = ObsConnectionParams::from_config(&cfg.connection).unwrap();
    let (event_tx, _event_rx) = mpsc::channel(8);
    let (obs_client, _, _, _disconnect) = connect(&params, event_tx).await.unwrap();

    let snapshot = ObsSnapshot {
        connected: true,
        ..ObsSnapshot::default()
    };

    let (client, shutdown) =
        start_test_server_with_obs_client(dir, cfg, obs_client, snapshot).await;
    (client, shutdown, fake_obs)
}

#[tokio::test]
async fn toggle_stream_returns_streaming_started_when_active() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown, fake_obs) = start_obs_connected_server(&dir).await;

    fake_obs
        .set_response(
            "ToggleStream",
            PreparedResponse::success(serde_json::json!({ "outputActive": true })),
        )
        .await;

    let resp = client
        .send_command(cmd("toggle_stream", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "toggle_stream should succeed");
    assert_eq!(result.unwrap()["message"], "streaming started");

    fake_obs.shutdown();
}

#[tokio::test]
async fn toggle_stream_returns_streaming_stopped_when_inactive() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown, fake_obs) = start_obs_connected_server(&dir).await;

    fake_obs
        .set_response(
            "ToggleStream",
            PreparedResponse::success(serde_json::json!({ "outputActive": false })),
        )
        .await;

    let resp = client
        .send_command(cmd("toggle_stream", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "toggle_stream should succeed");
    assert_eq!(result.unwrap()["message"], "streaming stopped");

    fake_obs.shutdown();
}

#[tokio::test]
async fn toggle_record_returns_recording_started_when_active() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown, fake_obs) = start_obs_connected_server(&dir).await;

    fake_obs
        .set_response(
            "ToggleRecord",
            PreparedResponse::success(serde_json::json!({ "outputActive": true })),
        )
        .await;

    let resp = client
        .send_command(cmd("toggle_record", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "toggle_record should succeed");
    assert_eq!(result.unwrap()["message"], "recording started");

    fake_obs.shutdown();
}

#[tokio::test]
async fn toggle_record_returns_recording_stopped_when_inactive() {
    let dir = TempDir::new().unwrap();
    let (mut client, _shutdown, fake_obs) = start_obs_connected_server(&dir).await;

    fake_obs
        .set_response(
            "ToggleRecord",
            PreparedResponse::success(serde_json::json!({ "outputActive": false })),
        )
        .await;

    let resp = client
        .send_command(cmd("toggle_record", Value::Null))
        .await
        .unwrap();
    let (ok, result, _) = extract_response(resp);
    assert!(ok, "toggle_record should succeed");
    assert_eq!(result.unwrap()["message"], "recording stopped");

    fake_obs.shutdown();
}

#[tokio::test]
async fn shutdown_ends_the_supervisor_while_a_connection_attempt_is_stalled() {
    // A fake OBS that accepts the socket and then never sends Hello: the
    // supervisor gets past the connect step and sits in the handshake.
    let mut fake_obs = spawn_silent_obs().await;

    let mut cfg = Config::default();
    cfg.connection.host = "127.0.0.1".to_string();
    cfg.connection.port = fake_obs.addr.port();
    cfg.connection.password_env = String::new();
    // Far longer than the test is willing to wait, so the handshake timeout
    // cannot be what ends the attempt — only the shutdown signal can.
    cfg.connection.connect_timeout_ms = 600_000;

    let hub = Arc::new(BroadcastHub::new());
    let state = StateStore::new(Arc::clone(&hub));
    let obs_handle: Arc<Mutex<Option<ObsClient>>> = Arc::new(Mutex::new(None));
    let (reconnect_tx, reconnect_rx) = mpsc::channel::<()>(4);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let supervisor = ObsSupervisor::new(ObsSupervisorConfig {
        config: Arc::new(Mutex::new(cfg)),
        state,
        obs_handle,
        reconnecting: Arc::new(AtomicBool::new(false)),
        reconnect_rx,
        shutdown: shutdown_rx,
        hub,
    });
    let supervising = tokio::spawn(supervisor.run());

    // Deterministic readiness: the attempt is provably in flight once the fake
    // server has a client on the other end of a completed WebSocket upgrade.
    fake_obs.wait_for_connection().await;
    shutdown_tx.send(true).unwrap();

    tokio::time::timeout(Duration::from_secs(5), supervising)
        .await
        .expect("shutdown must interrupt a stalled connection attempt")
        .expect("supervisor task should not panic");

    drop(reconnect_tx);
    fake_obs.shutdown();
}

// --- Issue #6: one client's slow command must not stop another client's ---

/// A daemon whose OBS can be made to hold one command open for as long as a
/// test likes.
///
/// Everything the daemon needs kept alive is parked here, so a test only has to
/// hold on to this one value: the OBS event channel (whose receiver going away
/// would end the OBS client's read task), the disconnect signal, and the
/// shutdown sender the IPC server watches.
struct HoldableObsDaemon {
    socket_path: PathBuf,
    fake_obs: support::fake_obs_server::FakeObsHandle,
    /// Releases the held `SetCurrentProgramScene`.
    gate: support::fake_obs_server::ResponseGate,
    _obs_events: mpsc::Receiver<obsctl_rs::obs::client::ObsEvent>,
    _obs_disconnected: tokio::sync::oneshot::Receiver<()>,
    _shutdown: watch::Sender<bool>,
}

impl HoldableObsDaemon {
    /// Start a daemon connected to a fake OBS that holds `SetCurrentProgramScene`
    /// — and therefore the `set_scene` command — until [`Self::gate`] is
    /// released.
    async fn start(dir: &TempDir) -> Self {
        let fake_obs = spawn_fake_obs(false, None).await;
        let (held, gate) = PreparedResponse::success(Value::Null).gated();
        fake_obs.set_response("SetCurrentProgramScene", held).await;

        let mut cfg = Config::default();
        cfg.connection.host = "127.0.0.1".to_string();
        cfg.connection.port = fake_obs.addr.port();
        cfg.connection.password_env = String::new();
        // Comfortably longer than these tests take. The held command has to
        // stay held until the test says otherwise, rather than until the OBS
        // client gives up waiting on the reply.
        cfg.connection.request_timeout_ms = 30_000;

        let params = ObsConnectionParams::from_config(&cfg.connection).unwrap();
        let (event_tx, obs_events) = mpsc::channel(8);
        let (obs_client, _, _, obs_disconnected) = connect(&params, event_tx).await.unwrap();

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

        let (socket_path, shutdown) =
            spawn_server_with_obs_client(dir, cfg, obs_client, snapshot).await;

        Self {
            socket_path,
            fake_obs,
            gate,
            _obs_events: obs_events,
            _obs_disconnected: obs_disconnected,
            _shutdown: shutdown,
        }
    }

    /// The `set_scene` command whose OBS request the gate holds.
    fn held_command() -> CommandPayload {
        cmd("set_scene", serde_json::json!({ "target": "Main" }))
    }

    /// Wait until the held command has actually reached OBS.
    ///
    /// This is the readiness handle that makes the tests below deterministic
    /// rather than timed: once the fake OBS reports the request, the command is
    /// known to be inside the daemon and stuck there until the gate opens.
    async fn wait_until_command_is_held(&mut self) {
        let deadline = Duration::from_secs(5);
        let request = tokio::time::timeout(deadline, async {
            loop {
                let (request_type, _) = self
                    .fake_obs
                    .requests
                    .recv()
                    .await
                    .expect("the fake OBS stopped reporting requests");
                if request_type == "SetCurrentProgramScene" {
                    return request_type;
                }
            }
        })
        .await;

        assert!(
            request.is_ok(),
            "the slow command never reached OBS, so it was never actually in flight"
        );
    }
}

/// The bug in issue #6: every connection's commands went through one loop that
/// ran them one at a time, so a command that takes seconds — `dump-config` does
/// two OBS round trips and rewrites the config file — stopped every other
/// client for the whole of it, including the TUI's polling.
///
/// The slow command here is held open at OBS rather than made slow by a sleep,
/// so the test does not race anything: the `ping` either comes back while the
/// other client's command is provably still running, or it does not come back
/// at all.
#[tokio::test]
async fn a_slow_command_does_not_hold_up_another_clients_command() {
    let dir = TempDir::new().unwrap();
    let mut daemon = HoldableObsDaemon::start(&dir).await;

    let mut slow_client = IpcClient::connect(&daemon.socket_path).await.unwrap();
    let mut fast_client = IpcClient::connect(&daemon.socket_path).await.unwrap();

    let slow = tokio::spawn(async move {
        slow_client
            .send_command(HoldableObsDaemon::held_command())
            .await
    });
    daemon.wait_until_command_is_held().await;

    let pong = tokio::time::timeout(
        Duration::from_secs(5),
        fast_client.send_command(cmd("ping", Value::Null)),
    )
    .await
    .expect("a second client's ping waited for the first client's held command")
    .expect("ping failed");
    let (ok, result, _) = extract_response(pong);
    assert!(ok, "ping should succeed while another client is busy");
    assert_eq!(result.unwrap()["message"], "pong");

    // Only now is the held command allowed to finish, which shows the ping
    // really did overtake a command that had not yet returned.
    daemon.gate.release();
    let (ok, result, code) = extract_response(slow.await.unwrap().unwrap());
    assert!(ok, "the held command should still succeed: {code:?}");
    assert_eq!(result.unwrap()["message"], "scene set: Main");

    daemon.fake_obs.shutdown();
}

/// The guarantee the fix must not trade away: two commands sent down one
/// connection run in the order that connection sent them.
///
/// `ping` and `get_server_status` never touch OBS, so nothing but the daemon's
/// own ordering can be keeping them behind the held `set_scene`. Spawning every
/// command onto its own task would answer both of them immediately and fail
/// this test.
#[tokio::test]
async fn one_clients_commands_run_in_the_order_it_sent_them() {
    let dir = TempDir::new().unwrap();
    let mut daemon = HoldableObsDaemon::start(&dir).await;

    let mut client = PipelinedClient::connect(&daemon.socket_path).await;
    client
        .send("cmd-1", HoldableObsDaemon::held_command())
        .await;
    client.send("cmd-2", cmd("ping", Value::Null)).await;
    client
        .send("cmd-3", cmd("get_server_status", Value::Null))
        .await;

    daemon.wait_until_command_is_held().await;

    let early = tokio::time::timeout(Duration::from_millis(500), client.next_response()).await;
    assert!(
        early.is_err(),
        "a later command from the same connection answered before the earlier one had finished: {early:?}"
    );

    daemon.gate.release();

    for expected in ["cmd-1", "cmd-2", "cmd-3"] {
        let answered = tokio::time::timeout(Duration::from_secs(5), client.next_response())
            .await
            .unwrap_or_else(|_| panic!("no response for {expected}"));
        match answered {
            ServerMessage::Response { id, ok, .. } => {
                assert_eq!(id, expected, "responses came back out of order");
                assert!(ok, "{expected} failed");
            }
            other => panic!("expected a response, got {other:?}"),
        }
    }

    daemon.fake_obs.shutdown();
}

/// An IPC client that sends commands without waiting for the answers.
///
/// `IpcClient` sends one command and reads until that command's response, which
/// cannot show what order the daemon *ran* things in. This writes all the
/// requests first and then reads the replies in the order they arrive on the
/// socket, which is exactly what the ordering test needs to see.
struct PipelinedClient {
    reader: tokio::io::BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl PipelinedClient {
    async fn connect(socket_path: &Path) -> Self {
        let stream = tokio::net::UnixStream::connect(socket_path).await.unwrap();
        let (reader, writer) = stream.into_split();
        Self {
            reader: tokio::io::BufReader::new(reader),
            writer,
        }
    }

    async fn send(&mut self, id: &str, command: CommandPayload) {
        use tokio::io::AsyncWriteExt;

        let frame = obsctl_rs::ipc::codec::encode(&ClientMessage::Command {
            id: id.to_string(),
            command,
        })
        .unwrap();
        self.writer.write_all(frame.as_bytes()).await.unwrap();
    }

    async fn next_response(&mut self) -> ServerMessage {
        use tokio::io::AsyncBufReadExt;

        let mut line = String::new();
        let read = self.reader.read_line(&mut line).await.unwrap();
        assert!(read > 0, "the daemon closed the connection");
        obsctl_rs::ipc::codec::decode::<ServerMessage>(&line).unwrap()
    }
}
