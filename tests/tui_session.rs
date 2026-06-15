// TUI model and event applier tests.

use obsctl_rs::{
    ipc::protocol::ServerMessage,
    obs::state::{AudioState, ObsSnapshot, SceneState},
    tui::{event_applier::apply_server_message, model::TuiModel},
};
use time::OffsetDateTime;

fn make_snapshot(connected: bool) -> ObsSnapshot {
    ObsSnapshot {
        connected,
        obs_studio_version: Some("30.0.0".into()),
        obs_websocket_version: Some("5.0.0".into()),
        current_scene: Some("Main".into()),
        scenes: vec![SceneState {
            name: "Main".into(),
            active: true,
            ..Default::default()
        }],
        audio_inputs: vec![AudioState {
            name: "Mic".into(),
            muted: Some(false),
            volume_percent: Some(80),
            ..Default::default()
        }],
        last_error: None,
        updated_at: OffsetDateTime::now_utc(),
    }
}

#[test]
fn state_event_updates_model_snapshot() {
    let mut model = TuiModel::default();
    assert!(!model.connected_to_daemon);

    let snapshot = make_snapshot(true);
    let data = serde_json::to_value(&snapshot).unwrap();
    let msg = ServerMessage::Event {
        topic: "state".into(),
        data,
    };

    apply_server_message(&mut model, msg);

    assert!(model.connected_to_daemon);
    assert!(model.snapshot.is_some());
    assert!(model.obs_connected());
    assert_eq!(model.current_scene(), Some("Main"));
    assert_eq!(model.scenes().len(), 1);
    assert_eq!(model.audio_inputs().len(), 1);
}

#[test]
fn log_event_appends_to_logs() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::Event {
        topic: "logs".into(),
        data: serde_json::json!({ "message": "something happened" }),
    };

    apply_server_message(&mut model, msg);

    assert_eq!(model.logs.len(), 1);
    assert_eq!(model.logs[0], "something happened");
}

#[test]
fn log_event_caps_at_200_entries() {
    let mut model = TuiModel::default();

    for i in 0..210 {
        let msg = ServerMessage::Event {
            topic: "logs".into(),
            data: serde_json::json!({ "message": format!("line {i}") }),
        };
        apply_server_message(&mut model, msg);
    }

    assert_eq!(model.logs.len(), 200);
    assert_eq!(model.logs[0], "line 10");
}

#[test]
fn unknown_topic_is_ignored() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::Event {
        topic: "events".into(),
        data: serde_json::json!({ "type": "SomeEvent" }),
    };

    apply_server_message(&mut model, msg);

    assert!(!model.connected_to_daemon);
    assert!(model.snapshot.is_none());
    assert!(model.logs.is_empty());
}

#[test]
fn malformed_state_payload_does_not_panic() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::Event {
        topic: "state".into(),
        data: serde_json::json!({ "not_a_snapshot": true }),
    };

    // Should not panic even with malformed data
    apply_server_message(&mut model, msg);
    // snapshot stays None because deserialization fails
    assert!(model.snapshot.is_none());
}

#[test]
fn model_helpers_return_empty_when_no_snapshot() {
    let model = TuiModel::default();
    assert_eq!(model.scenes().len(), 0);
    assert_eq!(model.audio_inputs().len(), 0);
    assert_eq!(model.current_scene(), None);
    assert!(!model.obs_connected());
}
