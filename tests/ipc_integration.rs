use std::sync::Arc;

use obsctl_rs::domain::errors::ObsctlError;
use obsctl_rs::ipc::{
    protocol::{
        CommandPayload, LogEvent, LogLevel, PublicErrorCode, ServerMessage, TOPIC_LOGS,
        TOPIC_STATE, public_error_code,
    },
    session::{BroadcastHub, CommandDispatch},
    unix_client::IpcClient,
    unix_server::IpcServer,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::{mpsc, watch},
};

async fn start_test_server(
    socket_path: &std::path::Path,
) -> (
    Arc<BroadcastHub>,
    mpsc::Receiver<CommandDispatch>,
    watch::Sender<bool>,
) {
    let hub = Arc::new(BroadcastHub::new());
    let (cmd_tx, cmd_rx) = mpsc::channel::<CommandDispatch>(64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = IpcServer::bind(socket_path, Arc::clone(&hub)).unwrap();
    let tx = cmd_tx.clone();
    tokio::spawn(async move { server.run(tx, shutdown_rx).await });
    (hub, cmd_rx, shutdown_tx)
}

#[tokio::test]
async fn logs_subscriber_receives_typed_log_event_json() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("logs.sock");
    let (hub, _cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client = IpcClient::connect(&socket_path).await.unwrap();
    client.subscribe(&[TOPIC_LOGS]).await.unwrap();

    hub.publish_log(LogEvent::new(LogLevel::Error, "OBS unavailable"));

    let event = client.next_event().await.unwrap();
    match event {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, TOPIC_LOGS);
            let log_event: LogEvent = serde_json::from_value(data).unwrap();
            assert_eq!(log_event.level, LogLevel::Error);
            assert_eq!(log_event.message, "OBS unavailable");
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[tokio::test]
async fn state_subscriber_does_not_receive_log_events() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("non_logs.sock");
    let (hub, _cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client = IpcClient::connect(&socket_path).await.unwrap();
    client.subscribe(&[TOPIC_STATE]).await.unwrap();

    hub.publish_log(LogEvent::new(LogLevel::Warn, "OBS unavailable"));

    let received =
        tokio::time::timeout(std::time::Duration::from_millis(50), client.next_event()).await;
    assert!(
        received.is_err(),
        "state subscriber should not receive logs"
    );
}

fn echo_handler(mut cmd_rx: mpsc::Receiver<CommandDispatch>) {
    tokio::spawn(async move {
        while let Some(dispatch) = cmd_rx.recv().await {
            let response = ServerMessage::Response {
                id: dispatch.id,
                ok: true,
                result: Some(serde_json::json!({ "message": "pong" })),
                error: None,
            };
            let _ = dispatch.reply.send(response);
        }
    });
}

#[tokio::test]
async fn command_round_trip() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("test.sock");
    let (_, cmd_rx, _shutdown) = start_test_server(&socket_path).await;
    echo_handler(cmd_rx);

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client = IpcClient::connect(&socket_path).await.unwrap();
    let resp = client
        .send_command(CommandPayload {
            name: "ping".to_string(),
            args: serde_json::Value::Null,
        })
        .await
        .unwrap();

    match resp {
        ServerMessage::Response { ok, result, .. } => {
            assert!(ok);
            assert_eq!(result.unwrap()["message"], "pong");
        }
        _ => panic!("expected Response"),
    }
}

#[tokio::test]
async fn multiple_commands_in_sequence() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("multi.sock");
    let (_, cmd_rx, _shutdown) = start_test_server(&socket_path).await;
    echo_handler(cmd_rx);

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client = IpcClient::connect(&socket_path).await.unwrap();
    for _ in 0..5 {
        let resp = client
            .send_command(CommandPayload {
                name: "ping".to_string(),
                args: serde_json::Value::Null,
            })
            .await
            .unwrap();
        assert!(matches!(resp, ServerMessage::Response { ok: true, .. }));
    }
}

#[tokio::test]
async fn subscribe_and_receive_broadcast() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("sub.sock");
    let (hub, mut cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    // Handler: respond to get_snapshot with an initial state event
    tokio::spawn(async move {
        while let Some(dispatch) = cmd_rx.recv().await {
            if dispatch.payload.name == "get_snapshot" {
                let snap = ServerMessage::Event {
                    topic: TOPIC_STATE.to_string(),
                    data: serde_json::json!({ "connected": false }),
                };
                let _ = dispatch.reply.send(snap);
            } else {
                let _ = dispatch.reply.send(ServerMessage::Response {
                    id: dispatch.id,
                    ok: true,
                    result: None,
                    error: None,
                });
            }
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client = IpcClient::connect(&socket_path).await.unwrap();
    client.subscribe(&[TOPIC_STATE]).await.unwrap();

    // Should receive initial state snapshot
    let event = client.next_event().await.unwrap();
    match &event {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, TOPIC_STATE);
            assert_eq!(data["connected"], false);
        }
        _ => panic!("expected Event"),
    }

    // Broadcast a state update
    hub.publish(
        TOPIC_STATE,
        ServerMessage::Event {
            topic: TOPIC_STATE.to_string(),
            data: serde_json::json!({ "connected": true }),
        },
    );

    let event2 = client.next_event().await.unwrap();
    match event2 {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, TOPIC_STATE);
            assert_eq!(data["connected"], true);
        }
        _ => panic!("expected Event"),
    }
}

#[tokio::test]
async fn invalid_topic_returns_error() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("invalid_topic.sock");
    let (_, _cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client = IpcClient::connect(&socket_path).await.unwrap();
    let result = client.subscribe(&["not_a_real_topic"]).await;
    let err = result.expect_err("invalid topic should be rejected");
    assert_eq!(public_error_code(&err).as_str(), "IPC_PROTOCOL_ERROR");
    match err {
        ObsctlError::IpcProtocolError(message) => {
            assert!(message.contains("unknown topics"));
            assert!(message.contains("not_a_real_topic"));
        }
        other => panic!("expected IpcProtocolError, got {other:?}"),
    }
}

#[tokio::test]
async fn raw_invalid_subscribe_returns_protocol_error_envelope() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("raw_invalid_topic.sock");
    let (_, _cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    writer
        .write_all(br#"{"id":"raw-invalid-001","type":"subscribe","topics":["not_a_real_topic"]}"#)
        .await
        .unwrap();
    writer.write_all(b"\n").await.unwrap();

    let mut line = String::new();
    let bytes_read = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        reader.read_line(&mut line),
    )
    .await
    .expect("server should write an invalid subscribe response")
    .unwrap();
    assert!(bytes_read > 0);
    assert!(line.ends_with('\n'), "response should be newline-delimited");

    let response: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    let envelope = response.as_object().expect("response should be an object");
    assert_eq!(
        envelope.len(),
        4,
        "unexpected response envelope: {response}"
    );
    assert_eq!(response["type"], "response");
    assert_eq!(response["id"], "raw-invalid-001");
    assert_eq!(response["ok"], false);
    assert!(
        !envelope.contains_key("result"),
        "error responses should omit result"
    );

    let error = response["error"]
        .as_object()
        .expect("error response should contain an error object");
    assert_eq!(error.len(), 2, "unexpected error payload: {response}");
    assert_eq!(
        error.get("code").and_then(serde_json::Value::as_str),
        Some(PublicErrorCode::IpcProtocolError.as_str())
    );
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .expect("error message should be a string");
    assert!(message.contains("unknown topics"));
    assert!(message.contains("not_a_real_topic"));
}

#[tokio::test]
async fn connection_refused_returns_error() {
    let path = std::path::Path::new("/tmp/obsctl_nonexistent_test.sock");
    let result = IpcClient::connect(path).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn multiple_clients_receive_broadcast() {
    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("multi_client.sock");
    let (hub, mut cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    // Handler: respond to get_snapshot with empty state
    tokio::spawn(async move {
        while let Some(dispatch) = cmd_rx.recv().await {
            let msg = ServerMessage::Event {
                topic: TOPIC_STATE.to_string(),
                data: serde_json::json!({}),
            };
            let _ = dispatch.reply.send(msg);
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let mut client1 = IpcClient::connect(&socket_path).await.unwrap();
    let mut client2 = IpcClient::connect(&socket_path).await.unwrap();

    client1.subscribe(&[TOPIC_STATE]).await.unwrap();
    client2.subscribe(&[TOPIC_STATE]).await.unwrap();

    // Consume initial snapshots
    client1.next_event().await.unwrap();
    client2.next_event().await.unwrap();

    // Broadcast
    hub.publish(
        TOPIC_STATE,
        ServerMessage::Event {
            topic: TOPIC_STATE.to_string(),
            data: serde_json::json!({ "broadcast": true }),
        },
    );

    let e1 = client1.next_event().await.unwrap();
    let e2 = client2.next_event().await.unwrap();
    for e in [e1, e2] {
        match e {
            ServerMessage::Event { data, .. } => {
                assert_eq!(data["broadcast"], true);
            }
            _ => panic!("expected Event"),
        }
    }
}
