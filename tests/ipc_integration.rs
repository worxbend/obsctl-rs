use std::sync::Arc;

use obsctl_rs::domain::errors::ObsctlError;
use obsctl_rs::ipc::{
    protocol::{
        CommandPayload, LogEvent, LogLevel, PublicErrorCode, ServerMessage, TOPIC_LOGS,
        TOPIC_STATE, Topic, public_error_code,
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
    time::timeout,
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

    let event = timeout(std::time::Duration::from_millis(500), client.next_event())
        .await
        .expect("timed out waiting for log event")
        .unwrap();
    match event {
        ServerMessage::Event { topic, data } => {
            assert_eq!(topic, Topic::Logs);
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

/// A client that never sends a newline must not be able to make the daemon
/// buffer whatever it likes.
///
/// The daemon owns the only connection to OBS, so its memory is everyone's
/// memory. This drives the abuse through a real socket and counts the bytes
/// the daemon was willing to take off it: a reader that stops at the frame
/// cap refuses the rest of the flood and hangs up, so the writes start
/// failing almost immediately. A reader that only checks the size once the
/// line is complete keeps swallowing bytes, and the count runs away to the
/// ceiling below.
#[tokio::test]
async fn a_never_ending_request_line_is_cut_off_near_the_frame_cap() {
    use obsctl_rs::ipc::protocol::MAX_IPC_LINE_BYTES;
    use tokio::io::AsyncReadExt;

    // Stop pushing at this point whatever happens, so a daemon that really is
    // willing to read forever fails the assertion instead of the test machine.
    const FLOOD_CEILING: usize = 64 * 1024 * 1024;
    // What a bounded reader may take: the 64 KiB frame cap plus however much
    // the kernel had already accepted into the socket buffers before the
    // daemon gave up. Generous, and still nowhere near the ceiling.
    const ACCEPTED_LIMIT: usize = 8 * 1024 * 1024;

    let dir = TempDir::new().unwrap();
    let socket_path = dir.path().join("flood.sock");
    let (_hub, _cmd_rx, _shutdown) = start_test_server(&socket_path).await;

    let mut stream = UnixStream::connect(&socket_path).await.unwrap();

    let chunk = vec![b'a'; MAX_IPC_LINE_BYTES];
    let mut accepted = 0usize;
    let flood = async {
        while accepted < FLOOD_CEILING {
            if stream.write_all(&chunk).await.is_err() {
                // The daemon hung up: exactly what should happen.
                break;
            }
            accepted += chunk.len();
        }
    };

    // A daemon that stopped reading without hanging up would block this write
    // forever; the timeout turns that into a reported failure too.
    timeout(std::time::Duration::from_secs(10), flood)
        .await
        .expect("the daemon neither read the flood nor closed the connection");

    assert!(
        accepted < ACCEPTED_LIMIT,
        "the daemon accepted {accepted} bytes of a single unterminated line; \
         the frame cap is {MAX_IPC_LINE_BYTES} bytes"
    );

    // And the outcome the client sees is unchanged: the session is dropped.
    let mut buf = [0u8; 1];
    let read = timeout(std::time::Duration::from_secs(2), stream.read(&mut buf))
        .await
        .expect("the daemon never closed the session")
        .unwrap_or(0);
    assert_eq!(read, 0, "expected the daemon to drop the client");
}
