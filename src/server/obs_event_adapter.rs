use crate::ipc::protocol::{InputMeterLevel, ObsEventPayload};
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
}
