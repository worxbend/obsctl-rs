// OBS WebSocket client integration tests — Phase 4.
//
// Uses FakeObsServer to exercise the obs::client handshake, request/response
// correlation, event dispatch, and error paths without requiring a real OBS.

mod support {
    #[allow(dead_code)]
    pub mod fake_obs_server;
}

use std::time::Duration;

use tokio::sync::mpsc;

use obsctl_rs::obs::client::{ObsEvent, handshake};
use obsctl_rs::obs::connection::ObsConnectionParams;
use obsctl_rs::obs::requests;
use support::fake_obs_server::{PreparedResponse, spawn_fake_obs};

// ── helpers ──────────────────────────────────────────────────────────────────

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn connect_to_fake(
    addr: std::net::SocketAddr,
    password: Option<&str>,
) -> (
    obsctl_rs::obs::client::ObsClient,
    String,
    String,
    mpsc::Receiver<ObsEvent>,
) {
    connect_to_fake_with_timeout(addr, password, 2500).await
}

async fn connect_to_fake_with_timeout(
    addr: std::net::SocketAddr,
    password: Option<&str>,
    request_timeout_ms: u64,
) -> (
    obsctl_rs::obs::client::ObsClient,
    String,
    String,
    mpsc::Receiver<ObsEvent>,
) {
    let url = format!("ws://{addr}");
    let ws_stream = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect")
        .0;
    let (sink, stream) = futures_util::StreamExt::split(ws_stream);
    let (ev_tx, ev_rx) = mpsc::channel(32);
    let (client, studio_ver, ws_ver, _disconnect) =
        handshake(sink, stream, password, ev_tx, request_timeout_ms)
            .await
            .expect("handshake");
    (client, studio_ver, ws_ver, ev_rx)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
fn unauthenticated_handshake_succeeds() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (_client, studio_ver, ws_ver, _ev) = connect_to_fake(server.addr, None).await;
        assert!(!studio_ver.is_empty());
        assert!(!ws_ver.is_empty());
        server.shutdown();
    });
}

#[test]
fn authenticated_handshake_succeeds_with_correct_password() {
    rt().block_on(async {
        let server = spawn_fake_obs(true, Some("supersecretpassword")).await;
        let (_client, studio_ver, ws_ver, _ev) =
            connect_to_fake(server.addr, Some("supersecretpassword")).await;
        assert_eq!(studio_ver, "30.0.0");
        assert!(!ws_ver.is_empty());
        server.shutdown();
    });
}

#[test]
fn authenticated_handshake_fails_with_wrong_password() {
    rt().block_on(async {
        let server = spawn_fake_obs(true, Some("correct")).await;

        let url = format!("ws://{}", server.addr);
        let ws_stream = tokio_tungstenite::connect_async(&url)
            .await
            .expect("ws connect")
            .0;
        let (sink, stream) = futures_util::StreamExt::split(ws_stream);
        let (ev_tx, _ev_rx) = mpsc::channel::<ObsEvent>(8);
        let result = handshake(sink, stream, Some("wrong"), ev_tx, 2500).await;
        assert!(result.is_err(), "expected error for wrong password");
        server.shutdown();
    });
}

#[test]
fn get_scene_list_request_returns_scenes() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        let result = client.request(requests::get_scene_list()).await;
        let data = result.expect("get_scene_list");
        let scenes = data
            .get("scenes")
            .and_then(|s| s.as_array())
            .expect("scenes array");
        assert!(!scenes.is_empty(), "expected at least one scene");
        server.shutdown();
    });
}

#[test]
fn get_current_program_scene_returns_scene_name() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        let result = client
            .request(requests::get_current_program_scene())
            .await
            .expect("get_current_program_scene");
        let name = result
            .get("currentProgramSceneName")
            .and_then(|v| v.as_str())
            .expect("scene name");
        assert_eq!(name, "Main");
        server.shutdown();
    });
}

#[test]
fn set_current_program_scene_request_is_dispatched() {
    rt().block_on(async {
        let mut server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        client
            .request(requests::set_current_program_scene("BRB").unwrap())
            .await
            .expect("set scene");

        // The request capture channel includes GetVersion from handshake; drain until we see it.
        let mut found = false;
        while let Ok(Some((req_type, _))) =
            tokio::time::timeout(Duration::from_millis(200), server.requests.recv()).await
        {
            if req_type == "SetCurrentProgramScene" {
                found = true;
                break;
            }
        }
        assert!(found, "SetCurrentProgramScene not dispatched");
        server.shutdown();
    });
}

#[test]
fn obs_request_failure_returns_error() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        server
            .set_response(
                "SetCurrentProgramScene",
                PreparedResponse::error("scene not found"),
            )
            .await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        let result = client
            .request(requests::set_current_program_scene("NonExistent").unwrap())
            .await;
        assert!(result.is_err(), "expected error from OBS");
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(
                msg.contains("scene not found") || msg.contains("OBS"),
                "unexpected error: {msg}"
            );
        }
        server.shutdown();
    });
}

#[test]
fn event_is_delivered_to_event_channel() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        server
            .push_event(
                "CurrentProgramSceneChanged",
                serde_json::json!({ "sceneName": "BRB" }),
            )
            .await;

        let (_, _, _, mut ev_rx) = connect_to_fake(server.addr, None).await;

        // The event should arrive within a short time
        let event = tokio::time::timeout(Duration::from_millis(500), ev_rx.recv())
            .await
            .expect("event timeout")
            .expect("no event received");

        match event {
            ObsEvent::CurrentProgramSceneChanged { scene_name } => {
                assert_eq!(scene_name, "BRB");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        server.shutdown();
    });
}

#[test]
fn get_input_list_returns_inputs() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        let data = client
            .request(requests::get_input_list())
            .await
            .expect("get_input_list");
        let inputs = data
            .get("inputs")
            .and_then(|v| v.as_array())
            .expect("inputs");
        assert!(!inputs.is_empty());
        server.shutdown();
    });
}

#[test]
fn get_input_mute_returns_mute_state() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        let data = client
            .request(requests::get_input_mute("Mic").unwrap())
            .await
            .expect("get_input_mute");
        assert!(data.get("inputMuted").is_some());
        server.shutdown();
    });
}

#[test]
fn multiple_requests_are_correlated_correctly() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        // Fire multiple requests concurrently
        let c1 = client.clone();
        let c2 = client.clone();
        let (r1, r2) = tokio::join!(
            c1.request(requests::get_scene_list()),
            c2.request(requests::get_input_list()),
        );

        assert!(r1.is_ok(), "scene list: {r1:?}");
        assert!(r2.is_ok(), "input list: {r2:?}");
        server.shutdown();
    });
}

// ── reconnect / disconnect race tests ─────────────────────────────────────────

#[test]
fn requests_fail_when_server_drops_connection() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;
        let (client, _, _, _) = connect_to_fake(server.addr, None).await;

        // Force-close all active connections while keeping the listener alive.
        server.disconnect_all();

        // Allow a moment for the TCP close to propagate.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Subsequent requests must fail, not hang forever.
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            client.request(requests::get_scene_list()),
        )
        .await;

        match result {
            Ok(Err(_)) => {} // expected: request fails with OBS_UNAVAILABLE or similar
            Ok(Ok(_)) => panic!("request should have failed after server dropped the connection"),
            Err(_) => panic!("request timed out — pending request map may not clean up on close"),
        }
        server.shutdown();
    });
}

#[test]
fn auth_string_not_exposed_in_error_messages() {
    rt().block_on(async {
        let server = spawn_fake_obs(true, Some("correct_pw")).await;

        let url = format!("ws://{}", server.addr);
        let ws_stream = tokio_tungstenite::connect_async(&url)
            .await
            .expect("ws connect")
            .0;
        let (sink, stream) = futures_util::StreamExt::split(ws_stream);
        let (ev_tx, _ev_rx) = mpsc::channel::<obsctl_rs::obs::client::ObsEvent>(8);

        let result =
            obsctl_rs::obs::client::handshake(sink, stream, Some("wrong_pw"), ev_tx, 2500).await;
        assert!(result.is_err());
        // The error message must not contain the wrong password
        let err_str = result.unwrap_err().to_string();
        assert!(
            !err_str.contains("wrong_pw"),
            "error message must not contain password: {err_str}"
        );
        server.shutdown();
    });
}

// ── reconnect after disconnect ────────────────────────────────────────────────

#[test]
fn new_connection_succeeds_after_previous_drops() {
    rt().block_on(async {
        let server = spawn_fake_obs(false, None).await;

        // First connection
        let (client1, _, _, _) = connect_to_fake(server.addr, None).await;

        // Disconnect all active connections.
        server.disconnect_all();

        // Allow the close to propagate.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // First client should be stale — requests return an error.
        let stale = tokio::time::timeout(
            Duration::from_secs(2),
            client1.request(requests::get_scene_list()),
        )
        .await;
        assert!(
            matches!(stale, Ok(Err(_)) | Err(_)),
            "stale client should error: {stale:?}"
        );

        // A new connection to the same server (listener still running) must succeed.
        let (client2, _, _, _) = connect_to_fake(server.addr, None).await;
        let r = client2.request(requests::get_scene_list()).await;
        assert!(r.is_ok(), "new client request after reconnect: {r:?}");

        server.shutdown();
    });
}

// ── request timeout ───────────────────────────────────────────────────────────

#[test]
fn request_timeout_fires_when_server_does_not_reply() {
    rt().block_on(async {
        // Use a very short timeout so the test completes quickly.
        const TIMEOUT_MS: u64 = 100;

        let server = spawn_fake_obs(false, None).await;
        // Tell the server to silently discard GetSceneList without replying.
        server
            .set_response("GetSceneList", PreparedResponse::no_reply())
            .await;

        let (client, _, _, _) = connect_to_fake_with_timeout(server.addr, None, TIMEOUT_MS).await;

        let start = std::time::Instant::now();
        let result = client.request(requests::get_scene_list()).await;
        let elapsed = start.elapsed();

        assert!(
            matches!(
                result,
                Err(obsctl_rs::domain::errors::ObsctlError::RequestTimeout)
            ),
            "expected RequestTimeout, got: {result:?}"
        );
        // Should fire close to the configured timeout (within a generous 200ms window).
        assert!(
            elapsed >= Duration::from_millis(TIMEOUT_MS),
            "timeout fired too early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_millis(TIMEOUT_MS + 200),
            "timeout fired too late: {elapsed:?}"
        );

        server.shutdown();
    });
}

#[test]
fn late_response_after_request_timeout_does_not_poison_next_request() {
    rt().block_on(async {
        const TIMEOUT_MS: u64 = 75;
        let late_response_delay = Duration::from_millis(TIMEOUT_MS + 150);

        let server = spawn_fake_obs(false, None).await;
        server
            .set_response(
                "SetCurrentProgramScene",
                PreparedResponse::success(serde_json::Value::Null).delayed(late_response_delay),
            )
            .await;

        let (client, _, _, _) = connect_to_fake_with_timeout(server.addr, None, TIMEOUT_MS).await;

        let timed_out = client
            .request(requests::set_current_program_scene("BRB").unwrap())
            .await;
        assert!(
            matches!(
                timed_out,
                Err(obsctl_rs::domain::errors::ObsctlError::RequestTimeout)
            ),
            "expected RequestTimeout, got: {timed_out:?}"
        );

        tokio::time::sleep(late_response_delay + Duration::from_millis(50)).await;

        let data = client
            .request(requests::get_scene_list())
            .await
            .expect("subsequent request after late timeout response");
        let scenes = data
            .get("scenes")
            .and_then(|s| s.as_array())
            .expect("scenes array");
        assert_eq!(scenes.len(), 2);

        server.shutdown();
    });
}

#[test]
fn connection_params_from_config_resolves_password_env() {
    use obsctl_rs::config::model::ConnectionConfig;

    // Safety: single-threaded test, no other threads reading this env var.
    unsafe { std::env::set_var("TEST_OBS_PW_RESOLVE", "mypassword") };
    let cfg = ConnectionConfig {
        host: "127.0.0.1".to_string(),
        port: 4455,
        password: None,
        password_env: "TEST_OBS_PW_RESOLVE".to_string(),
        connect_timeout_ms: 3000,
        request_timeout_ms: 2500,
        reconnect: None,
    };
    let params = ObsConnectionParams::from_config(&cfg).unwrap();
    assert_eq!(params.password.as_deref(), Some("mypassword"));
    unsafe { std::env::remove_var("TEST_OBS_PW_RESOLVE") };
}
