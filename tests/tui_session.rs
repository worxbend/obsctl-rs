// TUI model and event applier tests.

use obsctl_rs::{
    ipc::protocol::{LogEvent, LogLevel, ServerMessage, Topic},
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
        streaming: false,
        recording: false,
        updated_at: OffsetDateTime::now_utc(),
        ..ObsSnapshot::default()
    }
}

fn make_log(level: LogLevel, message: impl Into<String>) -> LogEvent {
    LogEvent {
        level,
        message: message.into(),
        target: Some("obsctl_rs::server".into()),
        timestamp: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn state_event_updates_model_snapshot() {
    let mut model = TuiModel::default();
    assert!(!model.connected_to_daemon);

    let snapshot = make_snapshot(true);
    let data = serde_json::to_value(&snapshot).unwrap();
    let msg = ServerMessage::Event {
        topic: Topic::State,
        data,
    };

    apply_server_message(&mut model, msg);

    assert!(model.connected_to_daemon);
    assert!(model.snapshot().is_some());
    assert!(model.obs_connected());
    assert_eq!(model.current_scene(), Some("Main"));
    assert_eq!(model.scenes().len(), 1);
    assert_eq!(model.audio_inputs().len(), 1);
}

#[test]
fn scene_change_starts_flash_but_first_snapshot_does_not() {
    let mut model = TuiModel::default();

    // First snapshot: no prior scene to compare against, so no flash.
    let first = make_snapshot(true);
    apply_server_message(
        &mut model,
        ServerMessage::Event {
            topic: Topic::State,
            data: serde_json::to_value(&first).unwrap(),
        },
    );
    assert!(model.scene_flash.is_none());

    // Switch scenes: flash should start for the newly active scene.
    let mut second = make_snapshot(true);
    second.current_scene = Some("Cam".into());
    second.scenes = vec![SceneState {
        name: "Cam".into(),
        active: true,
        ..Default::default()
    }];
    apply_server_message(
        &mut model,
        ServerMessage::Event {
            topic: Topic::State,
            data: serde_json::to_value(&second).unwrap(),
        },
    );
    assert_eq!(
        model.scene_flash.as_ref().map(|(name, _)| name.as_str()),
        Some("Cam")
    );

    // Same scene reported again: no new flash should be triggered (kept as-is).
    let started_at = model.scene_flash.clone();
    apply_server_message(
        &mut model,
        ServerMessage::Event {
            topic: Topic::State,
            data: serde_json::to_value(&second).unwrap(),
        },
    );
    assert_eq!(model.scene_flash, started_at);
}

#[test]
fn log_event_appends_to_logs() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::log_event(make_log(LogLevel::Warn, "something happened"));

    apply_server_message(&mut model, msg);

    assert_eq!(model.logs.len(), 1);
    assert_eq!(model.logs[0].level, LogLevel::Warn);
    assert_eq!(model.logs[0].message, "something happened");
    assert_eq!(model.logs[0].target.as_deref(), Some("obsctl_rs::server"));
    assert_eq!(model.logs[0].timestamp, OffsetDateTime::UNIX_EPOCH);
}

#[test]
fn log_event_caps_at_200_entries() {
    let mut model = TuiModel::default();

    for i in 0..210 {
        let msg = ServerMessage::log_event(make_log(LogLevel::Info, format!("line {i}")));
        apply_server_message(&mut model, msg);
    }

    assert_eq!(model.logs.len(), 200);
    assert_eq!(model.logs[0].message, "line 10");
}

#[test]
/// A log event the TUI cannot decode is not forwarded as if it were one, but
/// it is no longer dropped in silence either: the pane says one was skipped.
fn malformed_log_event_is_reported_not_forwarded() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::Event {
        topic: Topic::Logs,
        data: serde_json::json!({ "message": "missing level and timestamp" }),
    };

    apply_server_message(&mut model, msg);

    assert_eq!(model.logs.len(), 1, "expected exactly the skipped notice");
    let entry = &model.logs[0];
    assert_eq!(entry.level, LogLevel::Warn);
    assert!(
        !entry.message.contains("missing level and timestamp"),
        "the undecodable payload must not be passed off as a real log line"
    );
}

#[test]
/// An OBS event payload this build cannot decode changes nothing about the
/// model, but is surfaced so a TUI that has stopped tracking OBS says so.
fn undecodable_obs_event_touches_nothing_but_is_reported() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::Event {
        topic: Topic::Events,
        data: serde_json::json!({ "type": "SomeEvent" }),
    };

    apply_server_message(&mut model, msg);

    assert!(!model.connected_to_daemon);
    assert!(model.snapshot().is_none());
    assert_eq!(model.logs.len(), 1);
    assert_eq!(model.logs[0].level, LogLevel::Warn);
}

#[test]
fn malformed_state_payload_does_not_panic() {
    let mut model = TuiModel::default();

    let msg = ServerMessage::Event {
        topic: Topic::State,
        data: serde_json::json!({ "not_a_snapshot": true }),
    };

    // Should not panic even with malformed data
    apply_server_message(&mut model, msg);
    // snapshot stays None because deserialization fails
    assert!(model.snapshot().is_none());
    // ...and the user is told, rather than left looking at a frozen dashboard.
    assert_eq!(model.logs.len(), 1);
    assert_eq!(model.logs[0].level, LogLevel::Warn);
}

#[test]
fn model_helpers_return_empty_when_no_snapshot() {
    let model = TuiModel::default();
    assert_eq!(model.scenes().len(), 0);
    assert_eq!(model.audio_inputs().len(), 0);
    assert_eq!(model.current_scene(), None);
    assert!(!model.obs_connected());
}
