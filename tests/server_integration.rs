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
    config::{loader, model::Config},
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
        spawn_server_with_obs_client(dir, cfg, obs_client, snapshot, None).await;
    let client = IpcClient::connect(&socket_path).await.unwrap();
    (client, shutdown_tx)
}

/// The same daemon as [`start_test_server_with_obs_client`], handed back by its
/// socket path instead of by one connected client — which is what a test needs
/// when it wants two clients talking to the daemon at once.
///
/// `config_path` is what the commands that rewrite the config file work on;
/// `None` is the daemon a test gets when it only cares about OBS requests.
async fn spawn_server_with_obs_client(
    dir: &TempDir,
    cfg: Config,
    obs_client: ObsClient,
    snapshot: ObsSnapshot,
    config_path: Option<&Path>,
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
        config_path: config_path.map(Path::to_path_buf),
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

/// A daemon with a real [`ObsSupervisor`], so the snapshot under test is
/// filled in by an actual connect-and-refresh against the fake OBS rather than
/// seeded by the test.
///
/// `config_path` is the file the config-rewriting commands work on; `None` is
/// the daemon a test gets when it only cares about what OBS reports.
async fn start_test_server_with_obs_supervisor(
    dir: &TempDir,
    cfg: Config,
    config_path: Option<&Path>,
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
        config_path: config_path.map(Path::to_path_buf),
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
        start_test_server_with_obs_supervisor(&dir, cfg, None).await;
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
        start_test_server_with_obs_supervisor(&dir, cfg, None).await;
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
scene_profiles:
  # Hides nothing on purpose: a profile naming a scene the (empty) list above
  # does not mention is a validation warning, and the tests using this config
  # assert on what a clean load reports.
  - name: "streaming"
    hidden: []
active_scene_profile: ~
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

/// The config the scene-profile tests work on.
///
/// Two scenes whose per-scene `hidden` flags disagree with each other — that
/// is the baseline — and one scene profile that hides the *other* one. A
/// profile that only re-hid what the flags already hide could not tell the
/// two rules apart.
const SCENE_PROFILE_CONFIG_YAML: &str = r#"version: 1
connection:
  password_env: ""
scenes:
  - name: "Main"
    hidden: false
  - name: "Utility BG"
    hidden: true
scene_profiles:
  - name: "streaming"
    hidden:
      - "Main"
"#;

/// A daemon holding [`SCENE_PROFILE_CONFIG_YAML`], with the snapshot seeded as
/// if OBS had reported both scenes. Hands back the config file path, because
/// what these tests care about is what ends up written there.
async fn start_scene_profile_server(dir: &TempDir) -> (IpcClient, PathBuf, watch::Sender<bool>) {
    let config_path = dir.path().join("config.yml");
    std::fs::write(&config_path, SCENE_PROFILE_CONFIG_YAML).unwrap();

    let (client, state, shutdown) = start_test_server_with_config(dir, &config_path).await;
    state
        .seed_for_tests(ObsSnapshot {
            connected: true,
            scenes: vec![
                SceneState {
                    name: "Main".to_string(),
                    ..SceneState::default()
                },
                SceneState {
                    name: "Utility BG".to_string(),
                    ..SceneState::default()
                },
            ],
            ..ObsSnapshot::default()
        })
        .await;

    (client, config_path, shutdown)
}

/// What the config file itself says, loaded the way the daemon loads it.
///
/// The scene-profile commands are only useful if the change outlives the
/// process, so every one of these tests checks the file and not just the
/// daemon's answer.
fn read_config_from_disk(path: &Path) -> Config {
    loader::load_with_warnings(path)
        .expect("the config file the daemon just wrote must still load")
        .0
}

async fn send_ok(client: &mut IpcClient, name: &str, args: Value) -> Value {
    let resp = client.send_command(cmd(name, args)).await.unwrap();
    let (ok, result, code) = extract_response(resp);
    assert!(ok, "{name} failed with {code:?}");
    result.expect("a successful command answers with a result")
}

/// The `hidden` flag the snapshot reports for one scene.
fn scene_hidden(snapshot: &Value, name: &str) -> bool {
    snapshot["scenes"]
        .as_array()
        .expect("scenes array")
        .iter()
        .find(|scene| scene["name"] == name)
        .unwrap_or_else(|| panic!("no scene named {name} in the snapshot"))["hidden"]
        .as_bool()
        .expect("hidden is a bool")
}

#[tokio::test]
async fn save_scene_profile_writes_the_config_and_appears_in_the_snapshot() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    // Subscribed before the save, so the broadcast the save causes cannot be
    // missed. The initial snapshot pushed on subscribe arrives first and does
    // not carry the new profile, which is what the predicate skips past.
    let mut subscriber = IpcClient::connect(&dir.path().join("server_cfg.sock"))
        .await
        .unwrap();
    subscriber.subscribe(&[TOPIC_STATE]).await.unwrap();

    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({ "target": "podcast", "hidden": ["Utility BG"] }),
    )
    .await;
    assert_eq!(result["message"], "scene profile saved: podcast");
    assert_eq!(result["created"], true);
    assert_eq!(result["hidden"], 1);

    let on_disk = read_config_from_disk(&config_path);
    let saved = on_disk
        .scene_profiles
        .iter()
        .find(|profile| profile.name == "podcast")
        .expect("the saved scene profile must be in the file");
    assert_eq!(saved.hidden, vec!["Utility BG".to_string()]);
    assert_eq!(
        on_disk.scene_profiles.len(),
        2,
        "saving a new profile must not disturb the existing one"
    );
    assert!(
        on_disk.active_scene_profile.is_none(),
        "saving must not switch the saved profile on"
    );

    let state = next_state_event_matching(&mut subscriber, "the saved scene profile", |data| {
        data["scene_profiles"]
            .as_array()
            .is_some_and(|profiles| profiles.iter().any(|p| p["name"] == "podcast"))
    })
    .await;
    assert_eq!(state["active_scene_profile"], Value::Null);
}

#[tokio::test]
async fn activating_a_scene_profile_hides_exactly_its_scenes_in_the_snapshot() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    let result = send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;
    assert_eq!(result["message"], "scene profile set: streaming");
    assert_eq!(result["hidden"], 1);

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(snapshot["active_scene_profile"], "streaming");
    assert!(
        scene_hidden(&snapshot, "Main"),
        "the profile lists Main, so Main is hidden"
    );
    assert!(
        !scene_hidden(&snapshot, "Utility BG"),
        "an active profile replaces the per-scene flags rather than adding to \
         them, so a scene it omits is visible even though scenes: hides it"
    );

    assert_eq!(
        read_config_from_disk(&config_path).active_scene_profile,
        Some("streaming".to_string())
    );
}

#[tokio::test]
async fn clearing_the_scene_profile_restores_the_per_scene_baseline() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    let result = send_ok(&mut client, "clear_scene_profile", Value::Null).await;
    assert_eq!(result["message"], "scene profile cleared");

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(snapshot["active_scene_profile"], Value::Null);
    assert!(!scene_hidden(&snapshot, "Main"));
    assert!(scene_hidden(&snapshot, "Utility BG"));

    let on_disk = read_config_from_disk(&config_path);
    assert!(on_disk.active_scene_profile.is_none());
    assert_eq!(
        on_disk.scene_profiles.len(),
        1,
        "clearing switches a profile off, it does not delete it"
    );
}

/// The count a client puts on its status line has to be the number of rows the
/// user will watch disappear, not the number of lines the config file holds.
///
/// A profile's `hidden` list outlives the scenes it names: rename a scene in
/// OBS and its old spelling stays in the file, hiding nothing. Reporting the
/// entries made the TUI's status line say "hiding 2 scenes" while its own
/// badge — which counts the rows that actually went — said 1.
#[tokio::test]
async fn a_scene_profile_reports_the_scenes_it_really_hides_not_what_it_lists() {
    let dir = TempDir::new().unwrap();
    let (mut client, _config_path, _shutdown) = start_scene_profile_server(&dir).await;

    // "Renamed Away" is not one of the two scenes the daemon has been told
    // about, so it can never hide anything.
    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({
            "target": "podcast",
            "hidden": ["Utility BG", "Renamed Away"],
        }),
    )
    .await;
    assert_eq!(
        result["hidden"], 1,
        "only one of the two entries names a scene OBS has"
    );
    assert_eq!(
        result["listed"], 2,
        "and the file's own count is reported beside it, not instead of it"
    );

    let result = send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "podcast" }),
    )
    .await;
    assert_eq!(result["hidden"], 1);
    assert_eq!(result["listed"], 2);

    // Which is exactly what the snapshot then shows: one scene hidden.
    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert!(scene_hidden(&snapshot, "Utility BG"));
    assert!(!scene_hidden(&snapshot, "Main"));
}

/// A save reports whether it just changed the profile that is *in effect*.
///
/// Saving never switches a profile on, but editing the one already on
/// re-resolves the scene list as it writes — the rows move while the reply is
/// on its way back. A client that answered every save with "the active profile
/// is unchanged" was describing the pointer while the user watched the list.
#[tokio::test]
async fn a_save_says_whether_it_touched_the_profile_in_effect() {
    let dir = TempDir::new().unwrap();
    let (mut client, _config_path, _shutdown) = start_scene_profile_server(&dir).await;

    // Nothing is active yet, so this save moves nothing on screen.
    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({ "target": "streaming", "hidden": ["Main"] }),
    )
    .await;
    assert_eq!(result["active"], false);

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    // The same save now lands on the profile in effect, and the scene list
    // really does change under it.
    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({ "target": "streaming", "hidden": ["Utility BG"] }),
    )
    .await;
    assert_eq!(result["active"], true);
    assert_eq!(result["created"], false);

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert!(
        !scene_hidden(&snapshot, "Main"),
        "the scene the profile used to hide is back"
    );
    assert!(
        scene_hidden(&snapshot, "Utility BG"),
        "and the one it hides now is gone from the list"
    );

    // A rename carries `active_scene_profile` with it, so the renamed profile
    // is still the one in effect and the reply has to say so.
    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({
            "target": "night",
            "hidden": ["Utility BG"],
            "rename_from": "streaming",
        }),
    )
    .await;
    assert_eq!(result["renamed"], true);
    assert_eq!(result["active"], true);
}

/// A command that asks for a state the config file is already in must not
/// rewrite the file.
///
/// The write is not free: it re-serializes the whole config, replaces the file,
/// and moves its modification time, which wakes anything watching it for a
/// change that never happened. The check here is a comment line — `write_atomic`
/// rebuilds the file from the parsed model, so comments do not survive a
/// rewrite, and one that is still there is proof no rewrite happened.
#[tokio::test]
async fn a_scene_profile_command_that_changes_nothing_leaves_the_file_alone() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    // Marked after the daemon's own write, so only a *second* write can remove
    // it.
    const MARKER: &str = "# hand-written comment\n";
    let written = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(&config_path, format!("{MARKER}{written}")).unwrap();

    // Activating the profile that is already active, and clearing when there
    // is nothing to clear, are both legitimate requests — the caller asked for
    // a state and that state is already reached.
    let result = send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;
    assert_eq!(result["message"], "scene profile set: streaming");
    assert!(
        std::fs::read_to_string(&config_path)
            .unwrap()
            .starts_with(MARKER),
        "re-activating the active profile rewrote the config file"
    );

    // The command still works: the daemon and its clients agree with the file.
    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(snapshot["active_scene_profile"], "streaming");
    assert!(scene_hidden(&snapshot, "Main"));

    send_ok(&mut client, "clear_scene_profile", Value::Null).await;
    let cleared = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        !cleared.starts_with(MARKER),
        "clearing an active profile is a real change and must be written"
    );

    std::fs::write(&config_path, format!("{MARKER}{cleared}")).unwrap();
    send_ok(&mut client, "clear_scene_profile", Value::Null).await;
    assert!(
        std::fs::read_to_string(&config_path)
            .unwrap()
            .starts_with(MARKER),
        "clearing when nothing is active rewrote the config file"
    );

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(snapshot["active_scene_profile"], Value::Null);
    assert!(
        scene_hidden(&snapshot, "Utility BG"),
        "and the per-scene baseline is deciding again"
    );
}

#[tokio::test]
async fn deleting_the_active_scene_profile_also_clears_it() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    let result = send_ok(
        &mut client,
        "delete_scene_profile",
        serde_json::json!({ "target": "STREAMING" }),
    )
    .await;
    assert_eq!(result["message"], "scene profile deleted: streaming");
    assert_eq!(
        result["deactivated"], true,
        "the profile that was switched on has to be switched off with it"
    );

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(snapshot["active_scene_profile"], Value::Null);
    assert_eq!(snapshot["scene_profiles"], serde_json::json!([]));
    assert!(scene_hidden(&snapshot, "Utility BG"), "baseline is back");

    let on_disk = read_config_from_disk(&config_path);
    assert!(on_disk.scene_profiles.is_empty());
    assert!(on_disk.active_scene_profile.is_none());
}

/// The rename the TUI's editor performs, end to end: one command, and the
/// profile keeps the hold it had on the scene list.
///
/// Renaming used to be a save under the new name followed by a delete of the
/// old one, and deleting the active profile switches it off — so renaming the
/// profile in effect quietly stopped hiding anything.
#[tokio::test]
async fn renaming_the_active_scene_profile_keeps_it_active() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({
            "target": "night",
            "hidden": ["Main"],
            "rename_from": "streaming",
        }),
    )
    .await;
    assert_eq!(result["message"], "scene profile saved: night");
    assert_eq!(
        result["created"], false,
        "the entry moved, it was not added"
    );
    assert_eq!(result["renamed"], true);

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(snapshot["active_scene_profile"], "night");
    assert!(
        scene_hidden(&snapshot, "Main"),
        "the renamed profile still hides what it hid"
    );
    assert!(
        !scene_hidden(&snapshot, "Utility BG"),
        "and the per-scene baseline is still not the one deciding"
    );

    let on_disk = read_config_from_disk(&config_path);
    assert_eq!(on_disk.active_scene_profile, Some("night".to_string()));
    let names: Vec<&str> = on_disk
        .scene_profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect();
    assert_eq!(names, vec!["night"], "renaming leaves one profile, not two");
}

/// A rename that lands on a name a *different* profile already answers to is
/// refused. Doing it would replace that profile's hidden list with this one's
/// and then delete the entry being renamed away from — two profiles collapsing
/// into one, with no backup file to get the lost one back from.
#[tokio::test]
async fn renaming_a_scene_profile_onto_an_existing_name_is_refused() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({ "target": "podcast", "hidden": ["Utility BG"] }),
    )
    .await;

    let resp = client
        .send_command(cmd(
            "save_scene_profile",
            serde_json::json!({
                "target": "PODCAST",
                "hidden": ["Main"],
                "rename_from": "streaming",
            }),
        ))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok, "renaming onto an existing name must be refused");
    assert_eq!(code.as_deref(), Some("CONFIG_INVALID"));

    let on_disk = read_config_from_disk(&config_path);
    let podcast = on_disk
        .scene_profiles
        .iter()
        .find(|profile| profile.name == "podcast")
        .expect("the profile that was aimed at is still there");
    assert_eq!(
        podcast.hidden,
        vec!["Utility BG".to_string()],
        "and still hides what it hid"
    );
    assert_eq!(
        on_disk.scene_profiles.len(),
        2,
        "the profile being renamed is still there too"
    );
}

/// Saving over the *same* profile is not a rename, however differently it is
/// spelled, so it neither trips the collision check nor loses the active
/// pointer. A hand-written config can store a name with surrounding
/// whitespace, and the client passes that stored spelling back verbatim.
#[tokio::test]
async fn saving_a_scene_profile_under_its_own_name_is_not_a_rename() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    let result = send_ok(
        &mut client,
        "save_scene_profile",
        serde_json::json!({
            "target": "streaming",
            "hidden": ["Utility BG"],
            "rename_from": "  STREAMING  ",
        }),
    )
    .await;
    assert_eq!(result["created"], false);
    assert_eq!(result["renamed"], false);

    let on_disk = read_config_from_disk(&config_path);
    assert_eq!(on_disk.scene_profiles.len(), 1);
    assert_eq!(
        on_disk.scene_profiles[0].hidden,
        vec!["Utility BG".to_string()]
    );
    assert_eq!(
        on_disk.active_scene_profile,
        Some("streaming".to_string()),
        "the profile that was on is still on"
    );
}

/// A config file that is there but unparseable is left alone rather than
/// replaced with the daemon's startup copy. Unlike `dump-config`, a
/// scene-profile save writes no backup, so overwriting would destroy every
/// hand edit made since the daemon started with nothing to restore from.
#[tokio::test]
async fn a_scene_profile_save_refuses_to_overwrite_an_unreadable_config() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    let hand_edited = format!("{SCENE_PROFILE_CONFIG_YAML}      - \"unterminated\n");
    std::fs::write(&config_path, &hand_edited).unwrap();

    let resp = client
        .send_command(cmd(
            "save_scene_profile",
            serde_json::json!({ "target": "podcast", "hidden": ["Utility BG"] }),
        ))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok, "the save must not proceed onto a file it cannot read");
    assert_eq!(code.as_deref(), Some("CONFIG_INVALID"));

    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        hand_edited,
        "the file is byte-for-byte what the user left"
    );
}

#[tokio::test]
async fn set_scene_profile_for_an_unknown_name_returns_config_invalid() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    let resp = client
        .send_command(cmd(
            "set_scene_profile",
            serde_json::json!({ "target": "does-not-exist" }),
        ))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok);
    assert_eq!(code.as_deref(), Some("CONFIG_INVALID"));

    assert!(
        read_config_from_disk(&config_path)
            .active_scene_profile
            .is_none(),
        "a rejected edit must leave the file exactly as it was"
    );
}

#[tokio::test]
async fn save_scene_profile_rejects_an_oversized_hidden_list() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    // One past the 128-scene cap the executor enforces.
    let hidden: Vec<String> = (0..129).map(|index| format!("Scene {index}")).collect();

    let resp = client
        .send_command(cmd(
            "save_scene_profile",
            serde_json::json!({ "target": "too-big", "hidden": hidden }),
        ))
        .await
        .unwrap();
    let (ok, _, code) = extract_response(resp);
    assert!(!ok);
    assert_eq!(code.as_deref(), Some("COMMAND_PARSE_ERROR"));

    assert_eq!(
        read_config_from_disk(&config_path).scene_profiles.len(),
        1,
        "a malformed payload must not reach the file"
    );
}

#[tokio::test]
async fn list_scene_profiles_reports_what_the_config_holds() {
    let dir = TempDir::new().unwrap();
    let (mut client, _config_path, _shutdown) = start_scene_profile_server(&dir).await;

    // The test daemon starts holding a default config and only learns what is
    // in the file when something reads it, so read it first.
    send_ok(&mut client, "reload_config", Value::Null).await;

    let listing = send_ok(&mut client, "list_scene_profiles", Value::Null).await;
    assert_eq!(listing["active"], Value::Null);
    assert_eq!(
        listing["profiles"],
        serde_json::json!([{ "name": "streaming", "hidden": ["Main"] }])
    );

    send_ok(
        &mut client,
        "set_scene_profile",
        serde_json::json!({ "target": "streaming" }),
    )
    .await;

    let listing = send_ok(&mut client, "list_scene_profiles", Value::Null).await;
    assert_eq!(listing["active"], "streaming");
}

#[tokio::test]
async fn a_scene_profile_survives_dump_config() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.yml");
    std::fs::write(
        &config_path,
        SCENE_PROFILE_CONFIG_YAML.replace(
            "scene_profiles:",
            "active_scene_profile: streaming\nscene_profiles:",
        ),
    )
    .unwrap();

    let fake_obs = spawn_fake_obs(false, None).await;
    let mut cfg = Config::default();
    cfg.connection.host = "127.0.0.1".to_string();
    cfg.connection.port = fake_obs.addr.port();
    cfg.connection.password_env = String::new();
    cfg.connection.request_timeout_ms = 500;
    let params = ObsConnectionParams::from_config(&cfg.connection).unwrap();
    let (event_tx, _event_rx) = mpsc::channel(8);
    let (obs_client, _, _, _disconnect) = connect(&params, event_tx).await.unwrap();

    let (socket_path, _shutdown) = spawn_server_with_obs_client(
        &dir,
        cfg,
        obs_client,
        ObsSnapshot {
            connected: true,
            ..ObsSnapshot::default()
        },
        Some(&config_path),
    )
    .await;
    let mut client = IpcClient::connect(&socket_path).await.unwrap();

    send_ok(&mut client, "dump_config", Value::Null).await;

    // A dump rewrites the whole file from what OBS reports; scene profiles are
    // not something OBS knows about, so they have to ride through untouched.
    let on_disk = read_config_from_disk(&config_path);
    assert_eq!(on_disk.active_scene_profile, Some("streaming".to_string()));
    assert_eq!(on_disk.scene_profiles.len(), 1);
    assert_eq!(on_disk.scene_profiles[0].hidden, vec!["Main".to_string()]);
    assert!(
        on_disk.scenes.iter().any(|scene| scene.name == "BRB"),
        "the dump still recorded what OBS has"
    );
}

#[tokio::test]
async fn reload_config_picks_up_a_hand_edited_scene_profile() {
    let dir = TempDir::new().unwrap();
    let (mut client, config_path, _shutdown) = start_scene_profile_server(&dir).await;

    // Spelled in a different case than the profile it names, because a user
    // editing the file by hand is not required to match it exactly.
    std::fs::write(
        &config_path,
        format!("{SCENE_PROFILE_CONFIG_YAML}active_scene_profile: \"STREAMING\"\n"),
    )
    .unwrap();

    send_ok(&mut client, "reload_config", Value::Null).await;

    let snapshot = send_ok(&mut client, "get_snapshot", Value::Null).await;
    assert_eq!(
        snapshot["active_scene_profile"], "streaming",
        "the snapshot advertises the spelling the profile is stored under"
    );
    assert!(scene_hidden(&snapshot, "Main"));
    assert!(!scene_hidden(&snapshot, "Utility BG"));
}

// ── scene profiles, end to end ──────────────────────────────────────────────
//
// The tests above check one command at a time against a seeded snapshot. These
// check the sentence the feature exists to make true: *build a scene profile,
// switch it on, and the scene list is the scenes you chose*. So the scene list
// here is the one a real OBS reported over a real websocket handshake (the
// fake OBS server), the config is a real file the daemon rewrites, and every
// assertion is on the content of a snapshot that arrived on the `state` topic
// — not on the fact that a broadcast happened.

/// Six scenes: three a viewer is meant to see and three that only exist to be
/// nested inside the others. Two of the three utility scenes are hidden by the
/// baseline `scenes[].hidden` flags, so a profile switching off is visibly
/// different from no filtering at all.
const END_TO_END_SCENES: &[&str] = &[
    "Main",
    "Talking Head",
    "BRB",
    "Utility BG",
    "Overlay Src",
    "Lower Third",
];

/// The config file the end-to-end tests start from: no scene profiles at all,
/// which is where a user who has never used the feature starts.
fn end_to_end_config_yaml(obs_port: u16) -> String {
    format!(
        r#"version: 1
connection:
  host: "127.0.0.1"
  port: {obs_port}
  password_env: ""
  request_timeout_ms: 2000
scenes:
  - name: "Main"
  - name: "Talking Head"
  - name: "BRB"
  - name: "Utility BG"
    hidden: true
  - name: "Overlay Src"
    hidden: true
  - name: "Lower Third"
scene_profiles: []
"#
    )
}

/// A running daemon, its fake OBS, its config file, and two clients: one that
/// sends commands and one that subscribed to `state` before anything happened.
struct SceneProfileWorld {
    commands: IpcClient,
    /// Subscribed to `state` at startup, so no broadcast a test causes can be
    /// missed. Its first event is the snapshot pushed on subscribe.
    state: IpcClient,
    socket_path: PathBuf,
    config_path: PathBuf,
    shutdown: watch::Sender<bool>,
    fake_obs: support::fake_obs_server::FakeObsHandle,
}

impl SceneProfileWorld {
    /// Stop the daemon and the fake OBS. Both handles are explicit, so a test
    /// never has to wait out a timer to know they are gone.
    fn close(self) {
        let _ = self.shutdown.send(true);
        self.fake_obs.shutdown();
    }
}

/// Start a daemon whose scene list comes from OBS: fake OBS first (so its
/// scene list is in place before anything connects), then the config file,
/// then the daemon and its supervisor.
async fn start_end_to_end_world(dir: &TempDir) -> SceneProfileWorld {
    let fake_obs = spawn_fake_obs(false, None).await;
    fake_obs
        .set_response(
            "GetSceneList",
            PreparedResponse::success(serde_json::json!({
                "currentProgramSceneName": "Main",
                "scenes": END_TO_END_SCENES
                    .iter()
                    .enumerate()
                    .map(|(index, name)| serde_json::json!({
                        "sceneName": name,
                        "sceneIndex": index,
                    }))
                    .collect::<Vec<_>>(),
            })),
        )
        .await;

    let config_path = dir.path().join("config.yml");
    std::fs::write(&config_path, end_to_end_config_yaml(fake_obs.addr.port())).unwrap();
    // The daemon starts from the file it will later rewrite, exactly as it
    // does in production: `edit_config_file` rebuilds every write from what is
    // on disk, so an in-memory config that disagreed with the file would be
    // silently discarded at the first save.
    let cfg = loader::load_with_warnings(&config_path).unwrap().0;

    let (socket_path, _store, _hub, shutdown) =
        start_test_server_with_obs_supervisor(dir, cfg, Some(&config_path)).await;

    let commands = IpcClient::connect(&socket_path).await.unwrap();
    let mut state = IpcClient::connect(&socket_path).await.unwrap();
    state.subscribe(&[TOPIC_STATE]).await.unwrap();

    SceneProfileWorld {
        commands,
        state,
        socket_path,
        config_path,
        shutdown,
        fake_obs,
    }
}

/// The scene names a snapshot says are visible, in the order it lists them —
/// which is what the TUI's Scenes panel renders and what the user is really
/// asking about.
fn visible_scenes(snapshot: &Value) -> Vec<String> {
    snapshot["scenes"]
        .as_array()
        .expect("scenes array")
        .iter()
        .filter(|scene| !scene["hidden"].as_bool().expect("hidden is a bool"))
        .map(|scene| scene["name"].as_str().expect("scene name").to_string())
        .collect()
}

/// Wait until the supervisor's first refresh has landed, i.e. until a snapshot
/// carrying every scene OBS reports has been broadcast. Event-driven: the
/// client is already subscribed, so this reads what arrives rather than
/// polling for it.
async fn await_obs_scene_list(client: &mut IpcClient) -> Value {
    next_state_event_matching(client, "every scene the fake OBS reports", |data| {
        let Some(scenes) = data["scenes"].as_array() else {
            return false;
        };
        END_TO_END_SCENES
            .iter()
            .all(|wanted| scenes.iter().any(|scene| scene["name"] == *wanted))
    })
    .await
}

/// Wait for the snapshot in which `profile` is the active scene profile
/// (`None` for "no profile at all") and return what it says is visible.
async fn await_visible_under(
    client: &mut IpcClient,
    profile: Option<&str>,
) -> (Value, Vec<String>) {
    let expected = profile.map_or(Value::Null, |name| Value::String(name.to_string()));
    let description = match profile {
        Some(name) => format!("the scene profile `{name}` switched on"),
        None => "no scene profile switched on".to_string(),
    };
    let snapshot = next_state_event_matching(client, &description, |data| {
        data["active_scene_profile"] == expected
    })
    .await;
    let visible = visible_scenes(&snapshot);
    (snapshot, visible)
}

/// The two commands the TUI sends when the user builds a profile in the editor
/// and presses Enter: save it, then — because it is new — switch it on. See
/// `src/tui/daemon.rs::save_and_maybe_activate_scene_profile`.
async fn create_and_activate(client: &mut IpcClient, name: &str, hidden: &[&str]) {
    let saved = send_ok(
        client,
        "save_scene_profile",
        serde_json::json!({ "target": name, "hidden": hidden }),
    )
    .await;
    assert_eq!(
        saved["created"], true,
        "a name the config does not have yet must come back as a creation, \
         because that is the only thing that tells the TUI to switch it on"
    );
    send_ok(
        client,
        "set_scene_profile",
        serde_json::json!({ "target": name }),
    )
    .await;
}

/// The user's own sentence: make a profile, switch it on, and see only the
/// scenes it did not hide.
#[tokio::test]
async fn a_scene_profile_switched_on_leaves_exactly_its_unhidden_scenes_visible() {
    let dir = TempDir::new().unwrap();
    let mut world = start_end_to_end_world(&dir).await;

    let before = await_obs_scene_list(&mut world.state).await;
    assert_eq!(
        visible_scenes(&before),
        vec![
            "Main".to_string(),
            "Talking Head".to_string(),
            "BRB".to_string(),
            "Lower Third".to_string(),
        ],
        "before any profile exists, the per-scene `hidden` flags are what decides"
    );

    create_and_activate(
        &mut world.commands,
        "streaming",
        &["BRB", "Utility BG", "Overlay Src", "Lower Third"],
    )
    .await;

    let (snapshot, visible) = await_visible_under(&mut world.state, Some("streaming")).await;
    assert_eq!(
        visible,
        vec!["Main".to_string(), "Talking Head".to_string()],
        "the list the user is looking at is now exactly the scenes the profile left alone"
    );
    assert_eq!(
        snapshot["scene_profiles"]
            .as_array()
            .expect("the snapshot advertises the profiles")
            .len(),
        1
    );

    // And it is durable: a restart reads this back rather than the baseline.
    let on_disk = read_config_from_disk(&world.config_path);
    assert_eq!(on_disk.active_scene_profile, Some("streaming".to_string()));
    assert_eq!(
        on_disk.scene_profiles[0].hidden,
        vec![
            "BRB".to_string(),
            "Utility BG".to_string(),
            "Overlay Src".to_string(),
            "Lower Third".to_string(),
        ]
    );

    world.close();
}

/// Switching profiles is not "hide some more": the second profile's list
/// replaces the first one's, so a scene the old profile hid comes back and a
/// scene it showed goes away.
#[tokio::test]
async fn switching_scene_profiles_swaps_which_scenes_are_hidden() {
    let dir = TempDir::new().unwrap();
    let mut world = start_end_to_end_world(&dir).await;
    await_obs_scene_list(&mut world.state).await;

    create_and_activate(
        &mut world.commands,
        "streaming",
        &["BRB", "Utility BG", "Overlay Src", "Lower Third"],
    )
    .await;
    let (_, streaming) = await_visible_under(&mut world.state, Some("streaming")).await;
    assert_eq!(
        streaming,
        vec!["Main".to_string(), "Talking Head".to_string()]
    );

    // Saving a second profile does not switch it on — that is what `a` in the
    // picker and `P` on the dashboard are for — so the switch is its own step.
    send_ok(
        &mut world.commands,
        "save_scene_profile",
        serde_json::json!({
            "target": "break",
            "hidden": ["Main", "Talking Head", "Utility BG", "Overlay Src"],
        }),
    )
    .await;
    send_ok(
        &mut world.commands,
        "set_scene_profile",
        serde_json::json!({ "target": "break" }),
    )
    .await;

    let (_, on_break) = await_visible_under(&mut world.state, Some("break")).await;
    assert_eq!(
        on_break,
        vec!["BRB".to_string(), "Lower Third".to_string()],
        "the second profile's list replaces the first one's rather than adding to it"
    );
    for revealed in &on_break {
        assert!(
            !streaming.contains(revealed),
            "`{revealed}` was hidden a moment ago and is the proof the sets swapped"
        );
    }

    world.close();
}

/// Switching off is not the same as switching to a profile that hides nothing:
/// the per-scene `hidden` flags start deciding again, so the two scenes the
/// config marks hidden go back to being hidden.
#[tokio::test]
async fn clearing_the_scene_profile_puts_the_per_scene_baseline_back() {
    let dir = TempDir::new().unwrap();
    let mut world = start_end_to_end_world(&dir).await;
    await_obs_scene_list(&mut world.state).await;

    // This profile hides a scene the baseline shows and shows two the baseline
    // hides, so neither state can be mistaken for the other.
    create_and_activate(&mut world.commands, "everything-but-main", &["Main"]).await;
    let (_, under_profile) =
        await_visible_under(&mut world.state, Some("everything-but-main")).await;
    assert_eq!(
        under_profile,
        vec![
            "Talking Head".to_string(),
            "BRB".to_string(),
            "Utility BG".to_string(),
            "Overlay Src".to_string(),
            "Lower Third".to_string(),
        ],
        "an active profile replaces the per-scene flags, so it can reveal a scene too"
    );

    send_ok(&mut world.commands, "clear_scene_profile", Value::Null).await;

    let (snapshot, baseline) = await_visible_under(&mut world.state, None).await;
    assert_eq!(
        baseline,
        vec![
            "Main".to_string(),
            "Talking Head".to_string(),
            "BRB".to_string(),
            "Lower Third".to_string(),
        ],
        "with nothing switched on, `scenes[].hidden` is the answer again"
    );
    assert_eq!(
        snapshot["scene_profiles"]
            .as_array()
            .expect("the profiles are still listed")
            .len(),
        1,
        "switching off must not throw the profile away"
    );
    assert!(
        read_config_from_disk(&world.config_path)
            .active_scene_profile
            .is_none()
    );

    world.close();
}

/// A TUI started *after* the profile was switched on gets the filtered list in
/// the snapshot pushed on subscribe. Without this the feature would look like
/// it worked until the next restart.
#[tokio::test]
async fn a_client_that_connects_later_is_pushed_the_filtered_scene_list() {
    let dir = TempDir::new().unwrap();
    let mut world = start_end_to_end_world(&dir).await;
    await_obs_scene_list(&mut world.state).await;

    create_and_activate(
        &mut world.commands,
        "streaming",
        &["BRB", "Utility BG", "Overlay Src", "Lower Third"],
    )
    .await;
    await_visible_under(&mut world.state, Some("streaming")).await;

    let mut latecomer = IpcClient::connect(&world.socket_path).await.unwrap();
    latecomer.subscribe(&[TOPIC_STATE]).await.unwrap();

    // The *first* event, deliberately: this is the initial push, not a change
    // broadcast, and it is the only state a freshly started TUI has to draw
    // from until something in OBS moves.
    let initial = match next_event_with_timeout(&mut latecomer).await {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, Topic::State);
            data
        }
        other => panic!("expected the snapshot pushed on subscribe, got {other:?}"),
    };
    assert_eq!(initial["active_scene_profile"], "streaming");
    assert_eq!(
        visible_scenes(&initial),
        vec!["Main".to_string(), "Talking Head".to_string()],
        "the snapshot pushed on subscribe is already filtered"
    );

    world.close();
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
            spawn_server_with_obs_client(dir, cfg, obs_client, snapshot, None).await;

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
