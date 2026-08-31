use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};
use tracing::{debug, warn};

use crate::config::model::Config;
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::ipc::session::BroadcastHub;
use crate::obs::{
    client::{ObsClient, ObsEvent},
    requests,
    validation::extract_resource_names,
};
use crate::server::log_relay::ServerLog;
use crate::server::obs_event_adapter::{log_obs_event, needs_full_refresh, normalize_obs_event};
use crate::server::state_store::{
    ConfigProjection, Listing, ObsVersions, OutputFlags, RawInputState, RefreshedObsState,
    StateStore,
};
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// How many times the connect-path snapshot fetch will retry when events keep
/// landing mid-fetch before settling for the result it has.
const MAX_REFRESH_RETRIES: usize = 3;

/// One connection's ability to fetch OBS's whole state and publish it.
///
/// The connect path and the event-driven refresh worker do exactly the same
/// thing, so they hold one of these rather than each passing the same four
/// values around as four parameters.
pub(crate) struct SnapshotRefresher {
    config: Arc<Mutex<Config>>,
    state: StateStore,
    client: ObsClient,
    versions: ObsVersions,
}

impl SnapshotRefresher {
    /// Bundle one connection's fetch-and-publish dependencies.
    pub(crate) fn new(
        config: Arc<Mutex<Config>>,
        state: StateStore,
        client: ObsClient,
        versions: ObsVersions,
    ) -> Self {
        Self {
            config,
            state,
            client,
            versions,
        }
    }

    /// Fetch a full snapshot and publish it, reporting whether events landed
    /// while it was in flight — which means what was just published is already
    /// known to be behind and the caller should go round again.
    async fn refresh_once(&self) -> Result<bool> {
        // Read before the first round-trip: everything below describes OBS as it
        // was at some point after this, and any event that lands in between is
        // information this fetch does not have.
        let generation_before = self.state.generation();

        let scenes = fetch_scene_listing(&self.client).await?;
        let inputs = fetch_input_states(&self.client).await?;

        let refreshed = RefreshedObsState {
            versions: self.versions.clone(),
            scenes,
            inputs,
            profiles: fetch_listing(&self.client, ListingRequest::PROFILES).await,
            collections: fetch_listing(&self.client, ListingRequest::SCENE_COLLECTIONS).await,
            outputs: OutputFlags {
                streaming: fetch_output_active(
                    &self.client,
                    requests::get_stream_status(),
                    "GetStreamStatus",
                )
                .await,
                recording: fetch_output_active(
                    &self.client,
                    requests::get_record_status(),
                    "GetRecordStatus",
                )
                .await,
            },
        };

        let projection = {
            let cfg = self.config.lock().await;
            ConfigProjection::from_config(&cfg)
        };

        // The write lock keeps the swap atomic and preserves the metrics the stats
        // poller owns. It cannot keep this fetch current: an event applied while
        // the round-trips above were running is not in `refreshed` and has just
        // been overwritten. `apply_full_refresh` reports that so the caller can
        // fetch again rather than leave the wrong value in place.
        Ok(self
            .state
            .apply_full_refresh(&refreshed, &projection, generation_before)
            .await)
    }

    /// Fetch and publish a full snapshot, retrying while events keep landing
    /// mid-fetch.
    ///
    /// Bounded rather than a `while` loop: this runs on the connect path, and
    /// a busy OBS must not be able to keep it fetching instead of getting on
    /// with serving clients. Giving up leaves the same staleness the old code
    /// always had, so the bound costs nothing that was not already the case.
    pub(crate) async fn refresh_until_current(&self, log: &ServerLog) -> Result<()> {
        for _ in 0..MAX_REFRESH_RETRIES {
            if !self.refresh_once().await? {
                return Ok(());
            }
            debug!("OBS events landed during the snapshot fetch; refreshing again");
        }

        // Falling out of the loop is "we published a snapshot we already know
        // is behind, and stopped trying". That is a real, if rare, degraded
        // state — an operator looking at a stale scene list has no other way to
        // find out it is stale — so it is said out loud rather than returned as
        // an indistinguishable `Ok(())`.
        log.warn(format!(
            "OBS snapshot fetch gave up after {MAX_REFRESH_RETRIES} attempts; \
             published snapshot may be out of date"
        ));
        Ok(())
    }
}

/// Run one refresh per nudge on the returned channel, for as long as the
/// connection lives.
///
/// The channel has capacity 1 and senders use `try_send`: while a refresh is
/// running, a burst of list-changed events coalesces into exactly one follow-up
/// rather than queueing a refresh per event. The worker exits when the event
/// pump drops its sender, so it dies with the connection it belongs to.
///
/// The worker keeps a `Weak` handle on that same sender so a fetch that was
/// overtaken by events can queue a follow-up for itself. `Weak` rather than a
/// clone: an ordinary sender held here would keep the channel open forever and
/// the worker would never see the event pump let go.
pub(crate) fn spawn_refresh_worker(
    refresher: SnapshotRefresher,
    hub: Arc<BroadcastHub>,
) -> mpsc::Sender<()> {
    let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel::<()>(1);
    let refresh_self_tx = refresh_tx.downgrade();
    let log = ServerLog::new(hub, "obsctl_rs::server::obs_supervisor");

    tokio::spawn(async move {
        while refresh_rx.recv().await.is_some() {
            match refresher.refresh_once().await {
                // Events arrived while this fetch was in flight, so it
                // published values that are already out of date. Queue one
                // more pass; the capacity-1 channel coalesces repeats.
                Ok(true) => {
                    debug!("OBS events landed during a refresh; queueing another");
                    if let Some(tx) = refresh_self_tx.upgrade() {
                        let _ = tx.try_send(());
                    }
                }
                Ok(false) => {}
                Err(e) => log.warn(format!(
                    "OBS snapshot refresh after list change failed: {e}"
                )),
            }
        }
    });

    refresh_tx
}

/// Fold every OBS event into the cached snapshot and fan it out to clients,
/// asking for a full refresh when an event says a list has changed.
pub(crate) fn spawn_event_pump(
    mut event_rx: mpsc::Receiver<ObsEvent>,
    state: StateStore,
    hub: Arc<BroadcastHub>,
    refresh_tx: mpsc::Sender<()>,
) {
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let needs_full_refresh = needs_full_refresh(&event);

            // Log notable remote OBS changes before applying them.
            log_obs_event(&hub, &event);
            state.apply_event(event.clone()).await;
            if let Some(payload) = normalize_obs_event(&event) {
                hub.publish_obs_event(payload);
            }

            // Refreshing inline would block this loop for a dozen-plus
            // obs-websocket round-trips, back-pressuring the bounded event
            // channel and delaying every mute, volume, and scene change
            // queued behind it. A full channel means a refresh is already
            // pending, which is all the signal we need.
            if needs_full_refresh {
                let _ = refresh_tx.try_send(());
            }
        }
    });
}

/// Every scene OBS knows, plus the one currently on program.
///
/// Unlike the profile and scene-collection listings below, a failure here
/// fails the whole refresh: a snapshot without a scene list is not worth
/// publishing.
async fn fetch_scene_listing(client: &ObsClient) -> Result<Listing> {
    let scene_list_resp = client.request(requests::get_scene_list()).await?;
    let scenes = extract_resource_names(&scene_list_resp, "scenes", "sceneName")?;

    let current_resp = client
        .request(requests::get_current_program_scene())
        .await?;
    let current_scene = current_resp
        .get("currentProgramSceneName")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            warn!("Malformed GetCurrentProgramScene response: missing `currentProgramSceneName`");
            ObsctlError::ObsRequestFailed(
                "GetCurrentProgramScene response missing `currentProgramSceneName`".to_string(),
            )
        })?;
    let current_scene =
        trim_and_validate_token_with_max_len(current_scene, MAX_TARGET_TOKEN_LENGTH).map_err(
            |error| ObsctlError::ObsRequestFailed(format!("currentProgramSceneName {error}")),
        )?;

    if !scenes.contains(&current_scene) {
        warn!("Current program scene '{current_scene}' is not present in GetSceneList response");
    }

    Ok(Listing::new(scenes, current_scene))
}

/// The mute and volume of every audio input.
async fn fetch_input_states(client: &ObsClient) -> Result<Vec<RawInputState>> {
    let input_names = extract_resource_names(
        &client.request(requests::get_input_list()).await?,
        "inputs",
        "inputName",
    )?;

    // Each input needs two more round-trips. Issued serially this is the
    // dominant cost of a refresh — with ten inputs it was twenty sequential
    // waits — and every millisecond of it is time in which the snapshot being
    // assembled goes further out of date. `join_all` overlaps them instead.
    futures_util::future::join_all(
        input_names
            .iter()
            .map(|name| fetch_input_state(client, name)),
    )
    .await
    .into_iter()
    .collect()
}

/// Which obs-websocket request supplies a "list plus current selection", and
/// under which JSON keys its answer carries the two.
///
/// Profiles and scene collections are read exactly alike and differ only in
/// those names, so the difference lives in this table rather than in two
/// copies of [`fetch_listing`].
struct ListingRequest {
    build: fn() -> crate::obs::protocol::RequestData,
    request_name: &'static str,
    /// Key holding the array of names, e.g. `"profiles"`.
    list_key: &'static str,
    /// Key holding the currently selected name, e.g. `"currentProfileName"`.
    current_key: &'static str,
}

impl ListingRequest {
    const PROFILES: Self = Self {
        build: requests::get_profile_list,
        request_name: "GetProfileList",
        list_key: "profiles",
        current_key: "currentProfileName",
    };

    const SCENE_COLLECTIONS: Self = Self {
        build: requests::get_scene_collection_list,
        request_name: "GetSceneCollectionList",
        list_key: "sceneCollections",
        current_key: "currentSceneCollectionName",
    };
}

/// Read one "list plus current selection" from OBS.
///
/// A failed or malformed reply yields an empty listing rather than an error:
/// profiles and scene collections are secondary information, and losing them
/// should not cost the user the scene and audio state in the same snapshot.
async fn fetch_listing(client: &ObsClient, spec: ListingRequest) -> Listing {
    let Ok(response) = client.request((spec.build)()).await else {
        warn!("Missing {} response", spec.request_name);
        return Listing::new(Vec::new(), String::new());
    };

    let names = crate::obs::validation::extract_string_array(&response, spec.list_key)
        .unwrap_or_else(|_| {
            warn!(
                "Malformed {} response: missing `{}`",
                spec.request_name, spec.list_key
            );
            Vec::new()
        });
    let current = response
        .get(spec.current_key)
        .and_then(|s| s.as_str())
        .unwrap_or_else(|| {
            warn!(
                "Malformed {} response: missing `{}`",
                spec.request_name, spec.current_key
            );
            ""
        })
        .to_string();

    Listing::new(names, current)
}

/// Whether a stream or recording output is running.
///
/// Like [`fetch_listing`], an unreadable reply degrades rather than fails —
/// here to `false`, matching what the panel shows before OBS has answered.
async fn fetch_output_active(
    client: &ObsClient,
    request: crate::obs::protocol::RequestData,
    request_name: &str,
) -> bool {
    client
        .request(request)
        .await
        .ok()
        .and_then(|v| v.get("outputActive").and_then(|b| b.as_bool()))
        .unwrap_or_else(|| {
            warn!("Malformed {request_name} response: missing `outputActive`, defaulting to false");
            false
        })
}

/// Read one input's mute and volume.
///
/// A malformed or failed reply for a single field leaves that field `None`
/// rather than failing the whole refresh: one unreadable input should not cost
/// the user their scene list. Only a request that cannot be built at all — an
/// input name that fails validation — is an error.
async fn fetch_input_state(client: &ObsClient, name: &str) -> Result<RawInputState> {
    let mute_request = requests::get_input_mute(name)?;
    let volume_request = requests::get_input_volume(name)?;

    /// Request one field, warning — with the request and field spelled out —
    /// when the reply arrived but did not carry it in the expected shape.
    async fn field_or_warn<T>(
        client: &ObsClient,
        request: crate::obs::protocol::RequestData,
        request_name: &str,
        input_name: &str,
        field: &str,
        get: impl FnOnce(&serde_json::Value) -> Option<T>,
    ) -> Option<T> {
        let response = client.request(request).await.ok()?;
        let value = get(&response);
        if value.is_none() {
            warn!("Malformed {request_name} response for {input_name}: missing `{field}`");
        }
        value
    }

    let muted = field_or_warn(
        client,
        mute_request,
        "GetInputMute",
        name,
        "inputMuted",
        |v| v.get("inputMuted").and_then(|b| b.as_bool()),
    )
    .await;

    let volume_mul = field_or_warn(
        client,
        volume_request,
        "GetInputVolume",
        name,
        "inputVolumeMul",
        |v| v.get("inputVolumeMul").and_then(|f| f.as_f64()),
    )
    .await
    .filter(|volume_mul| {
        let usable = volume_mul.is_finite() && *volume_mul >= 0.0;
        if !usable {
            warn!(
                "Malformed GetInputVolume response for {name}: non-finite or negative `inputVolumeMul`"
            );
        }
        usable
    });

    Ok(RawInputState {
        name: name.to_string(),
        muted,
        volume_mul,
    })
}
