# obsctl-rs Plan

## Current Status

`obsctl-rs` is a Rust + Tokio + Ratatui OBS Studio controller with the intended daemon-first architecture:

```text
OBS Studio <---- obs-websocket 5.x ----> obsctl server <---- Unix socket IPC ----> obsctl tui
                                                               <---- Unix socket IPC ----> obsctl CLI
```

The normal-mode ownership rule remains mandatory: only `obsctl server` connects directly to OBS. CLI and TUI commands must stay thin IPC clients.

Implemented and verified:

- Config load/write/validation, legacy reconnect migration, dump-config merge, backups, and atomic writes.
- Shared command parser, alias/shortcut resolution, and volume conversion.
- Unix socket newline-delimited JSON IPC with typed request/response/event envelopes.
- OBS WebSocket 5.x handshake, auth, request/response correlation, event dispatch, and configured request timeout.
- Server daemon, state store, command executor, reconnect loop, IPC sessions, and systemd user service management.
- CLI local/proxy commands, server-unavailable UX, and stable proxy `--json` envelopes.
- Centralized public IPC error code taxonomy with documented CLI exit-code mapping.
- `REQUEST_TIMEOUT` is distinct from `OBS_UNAVAILABLE` in server IPC responses, CLI mapping, tests, and README docs.
- Ratatui TUI model/session/widgets with typed state and typed server log rendering.
- Typed IPC `LogEvent` contract with `level`, `message`, optional `target`, and RFC3339 `timestamp`.
- Explicit server-side typed log publication for selected daemon, supervisor, and command-executor events.
- Normalized typed OBS event IPC payloads for scene and audio events, published on the `events` topic while state updates continue on `state`.
- Shared best-effort redaction policy for IPC error/log messages, CLI proxy output, and structured JSON secret fields.

Latest verification:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features` with 221 tests passing

## Review Findings From Latest Iteration

Fully implemented:

- The `events` IPC topic is now real end to end for known OBS scene and audio events. `ObsSupervisor` applies each event to state and publishes a normalized `ObsEventPayload` to `TOPIC_EVENTS`.
- README, protocol fixtures, and server integration coverage now agree on normalized OBS event wire shapes such as `{"type":"CurrentProgramSceneChanged","scene_name":"BRB"}` and `InputMuteStateChanged`.
- Invalid subscription topics now have both typed-client and raw newline-delimited JSON assertions for `IPC_PROTOCOL_ERROR` and the documented unknown-topic message.
- Redaction logic has been consolidated into `support::redaction`; IPC errors/logs, CLI proxy output, and `support::json::redact_secrets` now share the same secret-key vocabulary.
- Redaction tests now cover Unicode-adjacent values, URL-encoded credentials, repeated redaction idempotence, mixed-case keys, bearer tokens, and structured `serde_json::Value` payloads.
- Fake OBS can broadcast events to active WebSocket clients, enabling server-path event publication tests instead of only hand-built protocol fixtures.
- Verification passes: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features`.

Partial or incomplete:

- The new `ObsEventPayload` contract lives in `ipc::protocol` but imports `obs::client::ObsEvent`, which couples the public IPC protocol module back to the OBS client implementation. A small domain event type or server-side adapter would keep dependency direction cleaner.
- Unknown OBS events are intentionally dropped from the public `events` topic. This keeps the public contract narrow, but it means clients cannot inspect vendor/new OBS events and the README should state that only known normalized events are published.
- The server-path event test proves routing, but it relies on the state subscriber's next event being the OBS-triggered state update. Because state subscriptions also send an initial snapshot asynchronously, the test should explicitly drain or match until the expected state to avoid order-sensitive flakiness.
- The normalized event payload currently includes only the fields already in `ObsEvent`. It does not include event timestamps, raw event names for aliased OBS events such as `SceneCreated`, or stable event IDs. That is acceptable for the current contract but should be revisited before broader client consumption.
- Redaction is unified, but still best-effort string scanning. Structured, typed non-secret fields remain safer than formatted messages containing secret-bearing values.
- Public error mapping tests rely on a manual variant-count constant. The exhaustive Rust matches are useful, but future maintainers still need a clearer pattern for updating variant fixtures intentionally.
- Fake OBS delayed responses currently sleep inside the connection handler before reading additional messages. That is adequate for a single late-response regression test, but it does not exercise concurrent late responses or timeout behavior under pipelined requests.
- Some server integration helpers still use fixed sleeps for readiness and shutdown.
- Log broadcasting is still manual and sparse. Ordinary `tracing` logs are not automatically bridged to IPC, so the TUI log panel does not yet represent the full server log stream promised by the product contract.
- `reload_config` updates command execution config and snapshot metadata, but changed OBS host/password/reconnect settings still need reconnect-path coverage. `dump-config` still does not reuse the same reload-and-rebroadcast path after writing.
- OBS supervisor still does not detect passive OBS disconnects by itself; it waits for shutdown or explicit reconnect while stale `ObsClient` handles can remain available until a later request fails.

Regressions or compatibility risks:

- Moving normalized event conversion into `ipc::protocol` makes the IPC layer depend on OBS-client types, increasing the chance that future OBS-client refactors accidentally alter the public protocol surface.
- The `events` topic now has an observable contract, so future payload-shape changes are wire compatibility changes and need tests/docs in the same iteration.
- The new server event routing test may pass or fail depending on whether the initial state snapshot or the event-induced state broadcast reaches the subscriber first.
- Boundary redaction is defense in depth, not a proof that arbitrary formatted messages are safe. Structured fields remain safer than secret-bearing strings.
- Background test tasks and fake IPC servers still lack deterministic shutdown/join handles in several helpers, so integration coverage can leak tasks and hide races.
- The repository still contains orchestration/planning artifacts whose intended ownership is unclear, including lowercase `plan.md` and JSONL telemetry files.

## Next Iteration Priorities

### P0: Tighten The New Event Contract

1. Decouple normalized IPC events from the OBS client module.
   - Move the public event payload model or the conversion adapter so `ipc::protocol` no longer imports `obs::client::ObsEvent`.
   - Keep `ObsEventPayload` as the public wire type and make the server/supervisor own conversion from internal OBS events.
   - Add a dependency-boundary check or focused review assertion so CLI/TUI remain thin IPC clients and IPC protocol types do not grow OBS implementation dependencies.

2. Make OBS event routing tests deterministic.
   - Drain the initial `state` subscription snapshot before asserting event-induced state updates, or loop until the expected snapshot is observed.
   - Replace fixed sleeps in the new supervisor test setup with readiness channels for IPC bind and OBS connected state.
   - Ensure the test proves `events` subscribers receive normalized payloads and `state` subscribers receive state snapshots without depending on broadcast ordering.

3. Clarify the public event coverage contract.
   - Document that only known normalized scene/audio events are published and unknown OBS events are intentionally dropped.
   - Decide whether `SceneCreated`, `SceneRemoved`, `SceneNameChanged`, and `SceneListReindexed` should all collapse to `SceneListChanged` publicly or preserve a normalized reason field.
   - Add protocol tests for every public `ObsEventPayload` variant, including `SceneListChanged`, `InputCreated`, and `InputRemoved`.
   - Consider whether event timestamps should be added now before clients depend on a timestamp-free contract.

4. Close remaining wire-fixture realism gaps.
   - Build at least one state event fixture from a real `ObsSnapshot` instead of ad hoc JSON.
   - Build log and OBS event compatibility fixtures through public constructors or server paths where practical.
   - Keep README examples synchronized with asserted fixtures.

### P1: Server Runtime Robustness

5. Detect passive OBS disconnects in `ObsSupervisor`.
   - Add an explicit connection-closed signal from the `ObsClient` task to supervisor.
   - Clear `obs_handle`, mark state disconnected, publish a typed log event, and enter reconnect delay without waiting for another command.
   - Add fake OBS server lifecycle coverage proving state changes when OBS closes the socket silently.

6. Harden request-timeout cleanup.
   - Cover multiple concurrent timeouts with late responses arriving out of order.
   - Cover disconnect during an in-flight timeout and prove pending requests complete promptly.
   - Update the fake OBS delayed-response path so one delayed response does not block reads for unrelated requests unless the test explicitly needs that behavior.
   - Avoid blocking `ObsClient::request` on cancellation bookkeeping if the client task is already gone.

7. Complete reload/dump refresh semantics.
   - After `dump-config`, reload config through the same path used by `reload_config_from_disk`.
   - Merge aliases/shortcuts into the current snapshot and rebroadcast state after dump.
   - Add tests for changed scene/audio aliases after dump and changed reconnect settings after reload plus explicit reconnect.

### P1: Logging Completeness

8. Bridge `tracing` to typed IPC log events.
   - Add a bounded, redacting tracing layer or server-local log sink that publishes `LogEvent`.
   - Keep manual high-value lifecycle events only where they add user-facing clarity.
   - Prevent log broadcast loops and avoid blocking tracing on slow IPC subscribers.
   - Add tests proving warnings/errors emitted through tracing reach `logs` subscribers and secrets are redacted.

9. Improve TUI log rendering ergonomics.
   - Truncate or elide long targets/messages so logs do not dominate narrow panels.
   - Consider hiding module targets by default or showing them only in debug mode.
   - Add widget assertions for long target names and mixed severity ordering.

### P2: Test and Repository Hygiene

10. Replace sleep-based test server readiness.
    - Add explicit readiness channels and shutdown handles for fake IPC and server integration helpers.
    - Join or abort background tasks/threads deterministically.
    - Remove duplicate test server setup where a shared helper can stay simple.

11. Clean planning and telemetry files.
    - Remove or intentionally document the untracked lowercase `plan.md`.
    - Decide whether `ALTERNATIVES.jsonl`, `SCORES.jsonl`, and similar orchestration artifacts belong in the repo.
    - Keep `PLAN.md`, `IMPLEMENTATION_CHECK_PLAN.md`, `AGENT_LOG.md`, and `MEMORY.md` roles distinct.

### P3: Product Expansion After Hardening

12. Recording control.
    - Add OBS requests: `StartRecord`, `StopRecord`, `PauseRecord`, `ResumeRecord`, `GetRecordStatus`.
    - Add CLI commands: `obsctl record start|stop|pause|resume|status`.
    - Add IPC command names and TUI header recording indicator.

13. Streaming control.
    - Add OBS requests: `StartStream`, `StopStream`, `GetStreamStatus`.
    - Add CLI commands and TUI status indicator.

14. Scene transition support.
    - Add transition list/status/set commands and state fields.
    - Preserve daemon-owned OBS access and typed IPC boundaries.

## Build Gates

Every implementation iteration must pass:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Before release candidates:

```sh
cargo build --release
```

Manual smoke test with OBS Studio:

```sh
cargo build
target/debug/obsctl init --force
export OBS_WEBSOCKET_PASSWORD='your password'
target/debug/obsctl validate-config
target/debug/obsctl server --headless
target/debug/obsctl server-status
target/debug/obsctl obs-status
target/debug/obsctl dump-config
target/debug/obsctl scene '<configured-alias-or-scene>'
target/debug/obsctl mute '<configured-audio>'
target/debug/obsctl unmute '<configured-audio>'
target/debug/obsctl vol '<configured-audio>' 70
target/debug/obsctl tui
```

## Durable Architecture Rules

- CLI and TUI must not import or call OBS WebSocket client code.
- Ratatui widgets render from `tui::TuiModel` only; no IPC or OBS work in widgets.
- Server command executor remains the only IPC-command-to-OBS-action path.
- Secrets must not appear in logs, errors, tests, snapshots, docs, or TUI panels.
- IPC contracts should be typed at the boundary and covered by wire-format tests before feature breadth expands.
- Public CLI/IPC behavior is a compatibility surface: changes to error codes, JSON envelopes, or topic payload shapes need docs and representative tests in the same iteration.
