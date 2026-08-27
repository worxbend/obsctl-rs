use crate::ipc::protocol::{InputMeterLevel, LogEvent, LogLevel, ObsEventPayload};
use crate::ipc::session::BroadcastHub;
use crate::obs::client::ObsEvent;

pub fn normalize_obs_event(event: &ObsEvent) -> Option<ObsEventPayload> {
    match event {
        ObsEvent::CurrentProgramSceneChanged { scene_name } => {
            Some(ObsEventPayload::CurrentProgramSceneChanged {
                scene_name: scene_name.clone(),
            })
        }
        ObsEvent::SceneListChanged => Some(ObsEventPayload::SceneListChanged),
        ObsEvent::InputCreated { input_name } => Some(ObsEventPayload::InputCreated {
            input_name: input_name.clone(),
        }),
        ObsEvent::InputRemoved { input_name } => Some(ObsEventPayload::InputRemoved {
            input_name: input_name.clone(),
        }),
        ObsEvent::InputMuteStateChanged { input_name, muted } => {
            Some(ObsEventPayload::InputMuteStateChanged {
                input_name: input_name.clone(),
                muted: *muted,
            })
        }
        ObsEvent::InputVolumeChanged {
            input_name,
            volume_mul,
            volume_db,
        } => Some(ObsEventPayload::InputVolumeChanged {
            input_name: input_name.clone(),
            volume_mul: *volume_mul,
            volume_db: *volume_db,
        }),
        ObsEvent::InputVolumeMeters { inputs } => Some(ObsEventPayload::InputVolumeMeters {
            inputs: inputs
                .iter()
                .map(|(name, level)| InputMeterLevel {
                    name: name.clone(),
                    level: *level,
                })
                .collect(),
        }),
        ObsEvent::StreamStateChanged { active } => {
            Some(ObsEventPayload::StreamStateChanged { active: *active })
        }
        ObsEvent::RecordStateChanged { active } => {
            Some(ObsEventPayload::RecordStateChanged { active: *active })
        }
        ObsEvent::CurrentProfileChanged { profile_name } => {
            Some(ObsEventPayload::CurrentProfileChanged {
                profile_name: profile_name.clone(),
            })
        }
        ObsEvent::ProfileListChanged => Some(ObsEventPayload::ProfileListChanged),
        ObsEvent::CurrentSceneCollectionChanged {
            scene_collection_name,
        } => Some(ObsEventPayload::CurrentSceneCollectionChanged {
            scene_collection_name: scene_collection_name.clone(),
        }),
        ObsEvent::SceneCollectionListChanged => Some(ObsEventPayload::SceneCollectionListChanged),
        ObsEvent::Other { .. } => None,
    }
}

/// Whether this event means the cached snapshot is missing whole entries, and
/// so has to be rebuilt from a full OBS fetch rather than edited in place.
///
/// OBS reports "the set of scenes changed" without saying what it changed to,
/// so there is nothing to apply: the only way to learn the new list is to ask
/// for it. Every other event carries its own new value and is folded into the
/// snapshot directly by `state_store::mutate_snapshot`.
///
/// Written as an exhaustive match with no `_ =>` arm on purpose. This used to
/// be a `matches!` against three variants inside the supervisor's event loop,
/// which quietly answered `false` for anything it had not been told about — so
/// a newly added list-changed event would have been converted to a public
/// payload, applied, and logged correctly, and simply never have triggered the
/// refresh it exists to trigger, with nothing failing to build. Spelling out
/// every variant turns that omission into a compile error instead.
pub fn needs_full_refresh(event: &ObsEvent) -> bool {
    match event {
        ObsEvent::SceneListChanged
        | ObsEvent::ProfileListChanged
        | ObsEvent::SceneCollectionListChanged => true,
        ObsEvent::CurrentProgramSceneChanged { .. }
        | ObsEvent::InputCreated { .. }
        | ObsEvent::InputRemoved { .. }
        | ObsEvent::InputMuteStateChanged { .. }
        | ObsEvent::InputVolumeChanged { .. }
        | ObsEvent::InputVolumeMeters { .. }
        | ObsEvent::StreamStateChanged { .. }
        | ObsEvent::RecordStateChanged { .. }
        | ObsEvent::CurrentProfileChanged { .. }
        | ObsEvent::CurrentSceneCollectionChanged { .. }
        | ObsEvent::Other { .. } => false,
    }
}

/// The one-line description an operator reads when this event arrives, or
/// `None` for the events not worth a line.
///
/// Plain English literals rather than `rust_i18n::t!` keys, deliberately. These
/// go out on the `logs` topic next to `tracing` output and the raw OBS error
/// strings the supervisor forwards, none of which are translated; making only
/// these translatable would give the TUI's log pane two languages at once.
///
/// This is the third projection of `ObsEvent` in this file, after
/// [`normalize_obs_event`] (event → public wire payload) and
/// [`needs_full_refresh`] (event → "must the snapshot be rebuilt?"). They live
/// together because adding a variant means answering all three questions, and
/// having to open one file to do it is the reminder that there are three.
pub fn describe_obs_event(event: &ObsEvent) -> Option<String> {
    match event {
        ObsEvent::CurrentProgramSceneChanged { scene_name } => {
            Some(format!("OBS: scene changed → {scene_name}"))
        }
        ObsEvent::SceneListChanged => Some("OBS: scene list changed".to_string()),
        ObsEvent::InputCreated { input_name } => Some(format!("OBS: input created: {input_name}")),
        ObsEvent::InputRemoved { input_name } => Some(format!("OBS: input removed: {input_name}")),
        ObsEvent::InputMuteStateChanged { input_name, muted } => {
            let state = if *muted { "muted" } else { "unmuted" };
            Some(format!("OBS: {input_name} {state}"))
        }
        ObsEvent::InputVolumeChanged {
            input_name,
            volume_db,
            ..
        } => {
            let db = if volume_db.is_finite() {
                format!("{volume_db:.1} dB")
            } else {
                "-∞ dB".to_string()
            };
            Some(format!("OBS: volume changed: {input_name} → {db}"))
        }
        ObsEvent::StreamStateChanged { active } => {
            let state = if *active { "started" } else { "stopped" };
            Some(format!("OBS: streaming {state}"))
        }
        ObsEvent::RecordStateChanged { active } => {
            let state = if *active { "started" } else { "stopped" };
            Some(format!("OBS: recording {state}"))
        }
        ObsEvent::CurrentProfileChanged { profile_name } => {
            Some(format!("OBS: profile changed → {profile_name}"))
        }
        ObsEvent::ProfileListChanged => Some("OBS: profile list changed".to_string()),
        ObsEvent::CurrentSceneCollectionChanged {
            scene_collection_name,
        } => Some(format!(
            "OBS: scene collection changed → {scene_collection_name}"
        )),
        ObsEvent::SceneCollectionListChanged => {
            Some("OBS: scene collection list changed".to_string())
        }
        // High-frequency or uninteresting — don't flood the log.
        ObsEvent::InputVolumeMeters { .. } | ObsEvent::Other { .. } => None,
    }
}

/// Publish [`describe_obs_event`]'s line on the `logs` topic.
pub fn log_obs_event(hub: &BroadcastHub, event: &ObsEvent) {
    if let Some(message) = describe_obs_event(event) {
        hub.publish_log(
            // Still named for the supervisor, which is where these events are
            // received and where this used to be written. The target travels
            // to clients inside the log payload, so it is renamed when the
            // thing producing the events moves, not when the formatting does.
            LogEvent::new(LogLevel::Info, message).with_target("obsctl_rs::server::obs_supervisor"),
        );
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn known_obs_events_create_public_payloads() {
        let payload = normalize_obs_event(&ObsEvent::InputVolumeChanged {
            input_name: "Desktop Audio".to_string(),
            volume_mul: 0.75,
            volume_db: -2.5,
        });

        assert_eq!(
            payload,
            Some(ObsEventPayload::InputVolumeChanged {
                input_name: "Desktop Audio".to_string(),
                volume_mul: 0.75,
                volume_db: -2.5,
            })
        );
    }

    #[test]
    fn unknown_obs_events_do_not_create_public_payloads() {
        let payload = normalize_obs_event(&ObsEvent::Other {
            event_type: "VendorSpecificEvent".to_string(),
            data: json!({ "raw": true }),
        });

        assert_eq!(payload, None);
    }

    #[test]
    fn stream_state_changed_creates_public_payload() {
        let payload = normalize_obs_event(&ObsEvent::StreamStateChanged { active: true });
        assert_eq!(
            payload,
            Some(ObsEventPayload::StreamStateChanged { active: true })
        );
    }

    #[test]
    fn record_state_changed_creates_public_payload() {
        let payload = normalize_obs_event(&ObsEvent::RecordStateChanged { active: false });
        assert_eq!(
            payload,
            Some(ObsEventPayload::RecordStateChanged { active: false })
        );
    }

    #[test]
    fn scene_collection_changed_creates_public_payload() {
        let payload = normalize_obs_event(&ObsEvent::CurrentSceneCollectionChanged {
            scene_collection_name: "Podcast".to_string(),
        });
        assert_eq!(
            payload,
            Some(ObsEventPayload::CurrentSceneCollectionChanged {
                scene_collection_name: "Podcast".to_string(),
            })
        );
    }

    #[test]
    fn scene_collection_list_changed_creates_public_payload() {
        let payload = normalize_obs_event(&ObsEvent::SceneCollectionListChanged);
        assert_eq!(payload, Some(ObsEventPayload::SceneCollectionListChanged));
    }

    /// The operator-facing wording, pinned. These lines are read by a person
    /// watching a stream go wrong, and they moved here from the supervisor
    /// unchanged; this is what says they are meant to stay that way.
    #[test]
    fn events_worth_a_log_line_describe_themselves() {
        let cases = [
            (
                ObsEvent::CurrentProgramSceneChanged {
                    scene_name: "Main".to_string(),
                },
                "OBS: scene changed → Main",
            ),
            (ObsEvent::SceneListChanged, "OBS: scene list changed"),
            (
                ObsEvent::InputMuteStateChanged {
                    input_name: "Mic".to_string(),
                    muted: true,
                },
                "OBS: Mic muted",
            ),
            (
                ObsEvent::InputMuteStateChanged {
                    input_name: "Mic".to_string(),
                    muted: false,
                },
                "OBS: Mic unmuted",
            ),
            (
                ObsEvent::InputVolumeChanged {
                    input_name: "Mic".to_string(),
                    volume_mul: 0.5,
                    volume_db: -6.02,
                },
                "OBS: volume changed: Mic → -6.0 dB",
            ),
            (
                ObsEvent::StreamStateChanged { active: true },
                "OBS: streaming started",
            ),
            (
                ObsEvent::RecordStateChanged { active: false },
                "OBS: recording stopped",
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(describe_obs_event(&event).as_deref(), Some(expected));
        }
    }

    /// A muted input reports a volume of negative infinity, which has no
    /// decimal form; it gets the symbol an audio meter would show.
    #[test]
    fn a_silent_input_is_described_as_negative_infinity() {
        let described = describe_obs_event(&ObsEvent::InputVolumeChanged {
            input_name: "Mic".to_string(),
            volume_mul: 0.0,
            volume_db: f64::NEG_INFINITY,
        });
        assert_eq!(
            described.as_deref(),
            Some("OBS: volume changed: Mic → -∞ dB")
        );
    }

    /// Volume meters arrive many times a second and vendor events mean nothing
    /// to a reader, so neither gets a line.
    #[test]
    fn high_frequency_and_unknown_events_are_not_described() {
        assert_eq!(
            describe_obs_event(&ObsEvent::InputVolumeMeters { inputs: Vec::new() }),
            None
        );
        assert_eq!(
            describe_obs_event(&ObsEvent::Other {
                event_type: "VendorSpecificEvent".to_string(),
                data: json!({ "raw": true }),
            }),
            None
        );
    }

    /// The three events that say "a set changed" without saying what it changed
    /// to. These are the only ones worth a dozen extra obs-websocket
    /// round-trips.
    #[test]
    fn list_changed_events_need_a_full_refresh() {
        for event in [
            ObsEvent::SceneListChanged,
            ObsEvent::ProfileListChanged,
            ObsEvent::SceneCollectionListChanged,
        ] {
            assert!(needs_full_refresh(&event), "{event:?} must force a refresh");
        }
    }

    /// Everything else carries its own new value, so the snapshot can be edited
    /// in place. Refreshing for these would put the daemon in a fetch loop
    /// during a volume fade, which emits events continuously.
    #[test]
    fn events_carrying_their_own_value_do_not_need_a_full_refresh() {
        for event in [
            ObsEvent::CurrentProgramSceneChanged {
                scene_name: "Main".to_string(),
            },
            ObsEvent::InputCreated {
                input_name: "Mic".to_string(),
            },
            ObsEvent::InputRemoved {
                input_name: "Mic".to_string(),
            },
            ObsEvent::InputMuteStateChanged {
                input_name: "Mic".to_string(),
                muted: true,
            },
            ObsEvent::InputVolumeChanged {
                input_name: "Mic".to_string(),
                volume_mul: 0.5,
                volume_db: -6.0,
            },
            ObsEvent::InputVolumeMeters { inputs: Vec::new() },
            ObsEvent::StreamStateChanged { active: true },
            ObsEvent::RecordStateChanged { active: false },
            ObsEvent::CurrentProfileChanged {
                profile_name: "Default".to_string(),
            },
            ObsEvent::CurrentSceneCollectionChanged {
                scene_collection_name: "Podcast".to_string(),
            },
            ObsEvent::Other {
                event_type: "VendorSpecificEvent".to_string(),
                data: json!({ "raw": true }),
            },
        ] {
            assert!(
                !needs_full_refresh(&event),
                "{event:?} must not force a refresh"
            );
        }
    }
}
