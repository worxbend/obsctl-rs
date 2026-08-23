use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, mpsc, watch};
use tracing::{debug, info, warn};

use crate::config::model::{Config, ReconnectConfig};
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::ipc::{protocol::LogLevel, session::BroadcastHub};
use crate::obs::{
    client::{ObsClient, ObsEvent},
    connection::{ObsConnectionParams, connect},
    requests,
    state::ObsStats,
    validation::extract_resource_names,
};
use crate::runtime::reconnect_policy::ReconnectPolicy;
use crate::server::log_relay::ServerLog;
use crate::server::obs_event_adapter::{log_obs_event, needs_full_refresh, normalize_obs_event};
use crate::server::state_store::{
    ConfigProjection, Listing, ObsVersions, OutputFlags, PolledMetrics, RawInputState,
    RefreshedObsState, StateStore,
};
use crate::support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len};

/// How many times the connect-path snapshot fetch will retry when events keep
/// landing mid-fetch before settling for the result it has.
const MAX_REFRESH_RETRIES: usize = 3;

pub struct ObsSupervisor {
    config: Arc<Mutex<Config>>,
    state: StateStore,
    obs_handle: Arc<Mutex<Option<ObsClient>>>,
    reconnecting: Arc<AtomicBool>,
    reconnect_rx: mpsc::Receiver<()>,
    shutdown: watch::Receiver<bool>,
    hub: Arc<BroadcastHub>,
    log: ServerLog,
}

/// Everything the supervisor needs to be handed at startup.
///
/// A struct rather than seven positional parameters, matching
/// `CommandExecutorConfig` next to it in the daemon's wiring: the daemon
/// constructs both one after the other, and reading two adjacent call sites
/// written in two different styles is harder than it needs to be. It also
/// means each value is named where it is passed.
pub struct ObsSupervisorConfig {
    pub config: Arc<Mutex<Config>>,
    pub state: StateStore,
    pub obs_handle: Arc<Mutex<Option<ObsClient>>>,
    pub reconnecting: Arc<AtomicBool>,
    pub reconnect_rx: mpsc::Receiver<()>,
    pub shutdown: watch::Receiver<bool>,
    pub hub: Arc<BroadcastHub>,
}

impl ObsSupervisor {
    pub fn new(cfg: ObsSupervisorConfig) -> Self {
        Self {
            config: cfg.config,
            state: cfg.state,
            obs_handle: cfg.obs_handle,
            reconnecting: cfg.reconnecting,
            reconnect_rx: cfg.reconnect_rx,
            shutdown: cfg.shutdown,
            log: ServerLog::new(Arc::clone(&cfg.hub), "obsctl_rs::server::obs_supervisor"),
            hub: cfg.hub,
        }
    }

    pub async fn run(mut self) {
        let mut reconnect_cfg = {
            let cfg = self.config.lock().await;
            cfg.reconnect.clone()
        };
        let mut policy = ReconnectPolicy::new(reconnect_cfg.clone());

        loop {
            match self.connect_once(&mut reconnect_cfg, &mut policy).await {
                ConnectOutcome::Connected(session) => match self.serve(session).await {
                    DisconnectReason::Shutdown => return,
                    // The connection was thrown away on purpose, not lost, so
                    // there is nothing to back off from: go straight round.
                    DisconnectReason::ReconnectRequested => continue,
                    DisconnectReason::ObsDisconnected => {}
                },
                ConnectOutcome::Failed(error) => self.record_connect_failure(&error).await,
                // The attempt was abandoned rather than failed, so there is
                // nothing to report and nothing to retry.
                ConnectOutcome::ShutdownRequested => {
                    self.reconnecting.store(false, Ordering::Relaxed);
                    return;
                }
            }

            if self.wait_before_retry(&mut policy).await.is_break() {
                return;
            }
        }
    }

    /// Read the current connection settings and try to open one connection.
    ///
    /// This is also where a config reload takes effect. The `reconnect`
    /// settings are re-read on every pass and the backoff policy rebuilt when
    /// they have changed, so editing them and running `reload-config` changes
    /// the next attempt rather than waiting for the daemon to be restarted.
    async fn connect_once(
        &mut self,
        reconnect_cfg: &mut ReconnectConfig,
        policy: &mut ReconnectPolicy,
    ) -> ConnectOutcome {
        let (params, latest_reconnect_cfg) = {
            let cfg = self.config.lock().await;
            (
                ObsConnectionParams::from_config(&cfg.connection),
                cfg.reconnect.clone(),
            )
        };

        // Raised before the attempt, not after it. Connecting takes up to
        // `connect_timeout_ms` (3s by default) plus an auth round-trip, and for
        // all of that time `server-status` used to report
        // `reconnecting: false, obs_connected: false` — the same thing it
        // reports once the supervisor has given up for good.
        self.reconnecting.store(true, Ordering::Relaxed);
        let attempt = match params {
            Ok(params) => self.attempt_connect_or_shutdown(&params).await,
            Err(error) => Some(Err(error)),
        };

        // Done whether the attempt succeeded or not: a reload that fixes the
        // backoff settings should apply to the retry that follows a failure,
        // which is exactly the case where it matters most.
        if latest_reconnect_cfg != *reconnect_cfg {
            *reconnect_cfg = latest_reconnect_cfg;
            *policy = ReconnectPolicy::new(reconnect_cfg.clone());
        }

        let session = match attempt {
            Some(Ok(session)) => session,
            Some(Err(error)) => return ConnectOutcome::Failed(error),
            None => return ConnectOutcome::ShutdownRequested,
        };
        let studio = &session.versions.studio;
        let websocket = &session.versions.websocket;
        self.log
            .info(format!("OBS connected: studio={studio} ws={websocket}"));
        policy.reset();
        self.reconnecting.store(false, Ordering::Relaxed);
        ConnectOutcome::Connected(session)
    }

    /// Try to connect, but give up the moment the daemon is asked to stop.
    ///
    /// `None` means the attempt was abandoned because shutdown was signalled;
    /// dropping the connect future is what cancels it. Without this race a
    /// connection attempt is uninterruptible for as long as it takes, and an
    /// attempt can take a while: `connect_timeout_ms` for the socket and the
    /// same again for the obs-websocket handshake. SIGTERM, SIGINT and the IPC
    /// `shutdown_server` command all arrive through this same watch channel,
    /// so all three used to be ignored until the attempt finished on its own.
    ///
    /// The receiver is cloned rather than borrowed so the attempt can keep
    /// using `&self` while the wait needs `&mut`; a clone starts out having
    /// seen exactly what the original has seen, so a shutdown that is already
    /// pending still wins the race.
    async fn attempt_connect_or_shutdown(
        &self,
        params: &ObsConnectionParams,
    ) -> Option<Result<ConnectedSession>> {
        let mut shutdown = self.shutdown.clone();
        tokio::select! {
            attempt = self.attempt_connect(params) => Some(attempt),
            // Also fires if every sender is gone, which means the daemon that
            // owns this supervisor is already on its way out.
            _ = shutdown.wait_for(|requested| *requested) => None,
        }
    }

    /// Record an attempt that never produced a connection, so that clients see
    /// why rather than only seeing `connected: false`.
    async fn record_connect_failure(&self, error: &ObsctlError) {
        warn!("OBS connection failed: {error}");
        self.log.warn(format!("OBS unavailable: {error}"));
        self.state.mark_disconnected(Some(error.to_string())).await;
    }

    /// Run one OBS connection for as long as it lasts, and report what ended
    /// it.
    ///
    /// Publishing the snapshot, handing the client to the command executor,
    /// waiting, and taking the client away again are one function because the
    /// taking-away has to happen on every way out. Spread across branches of
    /// the loop, it was one edit away from leaving `obs_handle` holding a
    /// client whose socket had already closed — which the executor would then
    /// keep handing to `scene`, `mute` and `vol`.
    async fn serve(&mut self, session: ConnectedSession) -> DisconnectReason {
        let ConnectedSession {
            client,
            versions,
            disconnect_rx,
        } = session;

        if let Err(e) = self.publish_initial_snapshot(&client, &versions).await {
            self.log
                .warn(format!("Initial OBS snapshot fetch failed: {e}"));
            // The fetch is what normally marks the snapshot connected, and it
            // did not run. The client below is installed either way, so leaving
            // the snapshot saying "disconnected" would have `obs-status` and
            // the TUI contradicting `scene`/`mute`/`vol`, which succeed against
            // exactly that client.
            self.state
                .mark_connected(
                    &versions,
                    Some(format!("initial snapshot fetch failed: {e}")),
                )
                .await;
        }

        *self.obs_handle.lock().await = Some(client);
        let reason = self.wait_for_disconnect(disconnect_rx).await;
        *self.obs_handle.lock().await = None;

        let (level, message, last_error) = reason.report();
        match level {
            LogLevel::Info => self.log.info(message),
            _ => self.log.warn(message),
        }
        self.state
            .mark_disconnected(last_error.map(str::to_string))
            .await;
        reason
    }

    /// Sit out the backoff before the next connection attempt.
    ///
    /// [`ControlFlow::Break`] means supervision is over for good, and is the
    /// one place `run` learns that: the config says not to reconnect, or the
    /// policy has run out of attempts, or the daemon is shutting down. The wait
    /// itself ends early when someone asks for a reconnect, so `obsctl
    /// reconnect` does not have to wait out a ten-second backoff.
    async fn wait_before_retry(&mut self, policy: &mut ReconnectPolicy) -> ControlFlow<()> {
        if !policy.enabled() {
            return self
                .stop_supervising(SupervisionEnd::ReconnectDisabled)
                .await;
        }

        loop {
            let Some(delay) = policy.next_delay() else {
                return self
                    .stop_supervising(SupervisionEnd::ReconnectExhausted)
                    .await;
            };
            info!("Reconnecting in {delay:?}");
            tokio::select! {
                _ = tokio::time::sleep(delay) => break,
                _ = self.reconnect_rx.recv() => {
                    self.log.info("OBS reconnect requested immediately");
                    break;
                }
                // A shutdown signal that is not yet `true` (the initial value
                // being observed) is not a reason to stop, so the wait starts
                // over — which draws a fresh, longer delay from the policy.
                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        // Deliberately no reason written to the snapshot: the
                        // daemon is stopping, so there is nothing an operator
                        // could act on, and `serve`'s shutdown branch already
                        // covers the case where a connection was live.
                        self.reconnecting.store(false, Ordering::Relaxed);
                        return ControlFlow::Break(());
                    }
                }
            }
        }

        if *self.shutdown.borrow() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    async fn attempt_connect(&self, params: &ObsConnectionParams) -> Result<ConnectedSession> {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<ObsEvent>(64);
        let (client, obs_version, ws_version, disconnect_rx) = connect(params, event_tx).await?;
        let versions = ObsVersions {
            studio: obs_version,
            websocket: ws_version,
        };

        spawn_stats_poller(client.clone(), self.state.clone());

        let refresh_tx = spawn_refresh_worker(
            self.snapshot_refresher(&client, &versions),
            Arc::clone(&self.hub),
        );
        spawn_event_pump(
            event_rx,
            self.state.clone(),
            Arc::clone(&self.hub),
            refresh_tx,
        );

        Ok(ConnectedSession {
            client,
            versions,
            disconnect_rx,
        })
    }

    fn snapshot_refresher(&self, client: &ObsClient, versions: &ObsVersions) -> SnapshotRefresher {
        SnapshotRefresher {
            config: Arc::clone(&self.config),
            state: self.state.clone(),
            client: client.clone(),
            versions: versions.clone(),
        }
    }

    /// Publish the snapshot a freshly opened connection is supposed to start
    /// life with.
    async fn publish_initial_snapshot(
        &self,
        client: &ObsClient,
        versions: &ObsVersions,
    ) -> Result<()> {
        self.snapshot_refresher(client, versions)
            .refresh_until_current(&self.log)
            .await
    }

    async fn wait_for_disconnect(
        &mut self,
        mut disconnect: tokio::sync::oneshot::Receiver<()>,
    ) -> DisconnectReason {
        loop {
            tokio::select! {
                _ = &mut disconnect => {
                    return DisconnectReason::ObsDisconnected;
                }
                _ = self.shutdown.changed() => {
                    if *self.shutdown.borrow() {
                        return DisconnectReason::Shutdown;
                    }
                }
                Some(()) = self.reconnect_rx.recv() => {
                    return DisconnectReason::ReconnectRequested;
                }
            }
        }
    }

    /// Leave a reason behind in the snapshot before supervision ends for good.
    ///
    /// Once `run` returns, nothing in the daemon will ever try OBS again, and
    /// `reconnect` has nobody to ask. Without this the only trace was a log
    /// line, which is gone by the time anyone looks: `obs-status`,
    /// `server-status.last_error` and the TUI's connection widget all read the
    /// snapshot, and it would still be showing whatever the last failed connect
    /// attempt happened to say. Writing the terminal reason there costs nothing
    /// on the wire — `last_error` is a field the snapshot already has.
    ///
    /// [`ControlFlow::Break`] is returned so each of the two endings in
    /// `wait_before_retry` is a single line.
    async fn stop_supervising(&self, end: SupervisionEnd) -> ControlFlow<()> {
        self.reconnecting.store(false, Ordering::Relaxed);
        match end.log_level() {
            LogLevel::Info => self.log.info(end.announcement()),
            _ => self.log.warn(end.announcement()),
        }
        self.state
            .mark_disconnected(Some(end.reason().to_string()))
            .await;
        ControlFlow::Break(())
    }
}

/// Why the supervisor will not try OBS again.
///
/// Both endings are announced to clients and then written into
/// `ObsSnapshot::last_error`, next to the OBS error strings the connect path
/// already puts there — so which one happened, how loudly to say it, and what an
/// operator is left looking at afterwards all live together here rather than
/// being re-assembled at each of the two call sites.
enum SupervisionEnd {
    /// The config says not to reconnect, so the first failure was the last.
    ReconnectDisabled,
    /// The backoff policy ran out of attempts.
    ReconnectExhausted,
}

impl SupervisionEnd {
    /// Configured behaviour is not a fault; running out of attempts is.
    fn log_level(&self) -> LogLevel {
        match self {
            Self::ReconnectDisabled => LogLevel::Info,
            Self::ReconnectExhausted => LogLevel::Warn,
        }
    }

    /// What is said on the `logs` topic as supervision ends.
    fn announcement(&self) -> &'static str {
        match self {
            Self::ReconnectDisabled => "OBS reconnect disabled; supervisor stopping",
            Self::ReconnectExhausted => "OBS reconnect exhausted; supervisor stopping",
        }
    }

    /// The operator diagnostic left in `ObsSnapshot::last_error`, which outlives
    /// the log line.
    fn reason(&self) -> &'static str {
        match self {
            Self::ReconnectDisabled => {
                "OBS reconnect disabled in config; supervisor stopped (restart the server to retry)"
            }
            Self::ReconnectExhausted => {
                "OBS reconnect attempts exhausted; supervisor stopped (restart the server to retry)"
            }
        }
    }
}

/// How one pass through the connect step ended.
///
/// A shutdown that interrupts an attempt is deliberately not an error: nothing
/// went wrong with OBS, so there is no failure to report to clients and no
/// backoff to sit out — the supervisor simply stops.
enum ConnectOutcome {
    Connected(ConnectedSession),
    Failed(ObsctlError),
    ShutdownRequested,
}

enum DisconnectReason {
    Shutdown,
    ReconnectRequested,
    ObsDisconnected,
}

impl DisconnectReason {
    /// How `serve` should report this ending: at what level, in what words, and
    /// with which `last_error` (if any) left behind in the snapshot.
    ///
    /// A planned shutdown gets `None`: the daemon is going away on purpose, so
    /// there is nothing for an operator to read about later.
    fn report(&self) -> (LogLevel, &'static str, Option<&'static str>) {
        match self {
            Self::Shutdown => (LogLevel::Info, "OBS disconnected during shutdown", None),
            Self::ReconnectRequested => (
                LogLevel::Warn,
                "OBS disconnected: reconnect requested",
                Some("reconnect requested"),
            ),
            Self::ObsDisconnected => (
                LogLevel::Warn,
                "OBS WebSocket closed",
                Some("OBS disconnected"),
            ),
        }
    }
}

/// One live OBS connection, everything the supervisor needs to work with it.
///
/// This used to be a four-element tuple whose middle two members were bare
/// `String`s — the OBS Studio version and the obs-websocket version — which
/// were then carried as two adjacent `&str` parameters through two more
/// functions before being put back together into the [`ObsVersions`] struct
/// that already existed. Two adjacent strings of the same type in a positional
/// list can be swapped without the compiler noticing; naming them once, in the
/// type that already models them, removes that possibility for the whole path.
struct ConnectedSession {
    client: ObsClient,
    versions: ObsVersions,
    /// Resolves when the WebSocket task exits, which is how the supervisor
    /// learns OBS went away rather than the daemon deciding to let go.
    disconnect_rx: tokio::sync::oneshot::Receiver<()>,
}

/// One connection's ability to fetch OBS's whole state and publish it.
///
/// The connect path and the event-driven refresh worker do exactly the same
/// thing, so they hold one of these rather than each passing the same four
/// values around as four parameters.
struct SnapshotRefresher {
    config: Arc<Mutex<Config>>,
    state: StateStore,
    client: ObsClient,
    versions: ObsVersions,
}

impl SnapshotRefresher {
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
    async fn refresh_until_current(&self, log: &ServerLog) -> Result<()> {
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
fn spawn_refresh_worker(refresher: SnapshotRefresher, hub: Arc<BroadcastHub>) -> mpsc::Sender<()> {
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
fn spawn_event_pump(
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

    let muted = client.request(mute_request).await.ok().and_then(|v| {
        match v.get("inputMuted").and_then(|b| b.as_bool()) {
            Some(muted) => Some(muted),
            None => {
                warn!("Malformed GetInputMute response for {name}: missing `inputMuted`");
                None
            }
        }
    });

    let volume_mul = client
        .request(volume_request)
        .await
        .ok()
        .and_then(
            |v| match v.get("inputVolumeMul").and_then(|f| f.as_f64()) {
                Some(volume_mul) if volume_mul.is_finite() && volume_mul >= 0.0 => Some(volume_mul),
                None => {
                    warn!("Malformed GetInputVolume response for {name}: missing `inputVolumeMul`");
                    None
                }
                Some(_) => {
                    warn!(
                        "Malformed GetInputVolume response for {name}: non-finite or negative `inputVolumeMul`"
                    );
                    None
                }
            },
        );

    Ok(RawInputState {
        name: name.to_string(),
        muted,
        volume_mul,
    })
}

const STATS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Poll `GetStats`/`GetStreamStatus`/`GetRecordStatus` on a fixed interval
/// and publish the results via `StateStore::update_stats`. Stream bitrate
/// isn't available directly from obs-websocket, so it's derived from the
/// delta of `GetStreamStatus.outputBytes` between polls (the same approach
/// OBS's own stats dock and most third-party remotes use).
fn spawn_stats_poller(client: ObsClient, state: StateStore) {
    tokio::spawn(async move {
        let mut last_bytes: Option<(u64, tokio::time::Instant)> = None;
        let mut interval = tokio::time::interval(STATS_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            let Ok(stats_resp) = client.request(requests::get_stats()).await else {
                // Client is most likely disconnected; the supervisor's main
                // loop will notice and drop this poller along with it.
                break;
            };
            let stats = ObsStats::from_response(&stats_resp);

            let stream_resp = client.request(requests::get_stream_status()).await.ok();
            let bitrate_kbps = stream_resp.as_ref().and_then(|v| {
                let active = v
                    .get("outputActive")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                let bytes = v.get("outputBytes").and_then(|b| b.as_u64())?;
                if !active {
                    last_bytes = None;
                    return None;
                }
                let now = tokio::time::Instant::now();
                let kbps = last_bytes.and_then(|(prev_bytes, prev_time)| {
                    if bytes < prev_bytes {
                        return None; // stream (re)started; skip this sample
                    }
                    let elapsed = now.duration_since(prev_time).as_secs_f64();
                    (elapsed > 0.0).then(|| (bytes - prev_bytes) as f64 * 8.0 / 1000.0 / elapsed)
                });
                last_bytes = Some((bytes, now));
                kbps
            });
            let stream_duration_ms = stream_resp.as_ref().and_then(output_duration_if_active);

            let record_resp = client.request(requests::get_record_status()).await.ok();
            let record_duration_ms = record_resp.as_ref().and_then(output_duration_if_active);

            state
                .update_stats(PolledMetrics::new(
                    stats,
                    bitrate_kbps,
                    stream_duration_ms,
                    record_duration_ms,
                ))
                .await;
        }
    });
}

/// Extract `outputDuration` from a `GetStreamStatus`/`GetRecordStatus`
/// response, but only while `outputActive` is true (OBS keeps the last
/// duration around after stopping, which would otherwise look live).
fn output_duration_if_active(response: &serde_json::Value) -> Option<u64> {
    let active = response
        .get("outputActive")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    if !active {
        return None;
    }
    response.get("outputDuration").and_then(|d| d.as_u64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_duration_if_active_returns_none_when_inactive() {
        let v = serde_json::json!({ "outputActive": false, "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), None);
    }

    #[test]
    fn output_duration_if_active_returns_duration_when_active() {
        let v = serde_json::json!({ "outputActive": true, "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), Some(5000));
    }

    #[test]
    fn output_duration_if_active_defaults_missing_active_to_false() {
        let v = serde_json::json!({ "outputDuration": 5000 });
        assert_eq!(output_duration_if_active(&v), None);
    }
}
