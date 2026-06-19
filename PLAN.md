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
- Normalized typed OBS event IPC payloads for known scene and audio events on the `events` topic while state updates continue on `state`.
- Shared best-effort redaction policy for IPC error/log messages, CLI proxy output, and structured JSON secret fields.

Latest verification:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features` with 225 tests passing

## Review Findings From Latest Iteration

Fully implemented:

- OBS-to-public-event conversion now lives in `server::obs_event_adapter`; the `domain::events` cross-layer adapter was removed.
- `ObsSupervisor` owns the conversion boundary: it applies the internal `ObsEvent` to state, then publishes an already-normalized `ObsEventPayload` only when the event is part of the public contract.
- Dependency-boundary tests now cover all production IPC modules for OBS implementation imports and all production domain modules for IPC protocol or OBS-client imports.
- Boundary guard matcher tests cover direct, package-qualified, bare, and grouped import forms such as `crate::obs::{client::ObsEvent}` and `obsctl_rs::{ipc::{protocol::ServerMessage}}`.
- Server-path event coverage now exercises every public `ObsEventPayload` variant through fake OBS broadcasts: `CurrentProgramSceneChanged`, `SceneListChanged`, `InputCreated`, `InputRemoved`, `InputMuteStateChanged`, and `InputVolumeChanged`.
- Unknown OBS events are covered through the real fake-OBS-to-IPC path and do not reach `events` subscribers before a subsequent known-event barrier.
- The event-routing log negative assertion no longer uses a fixed quiet window; the test drains log events until an injected marker and checks the drained set.
- Verification passes: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features` with 225 tests.

Partial or incomplete:

- The new boundary scanner only inspects `use` imports. Fully-qualified production references such as `crate::obs::client::ObsEvent` in type signatures or call sites can still bypass the guard.
- Boundary-scanning code is duplicated across `tests/architecture_boundaries.rs` and `tests/dependency_boundaries.rs`, while an older literal-string thin-client check remains in `tests/server_integration.rs`.
- The server-path event test is now broad but too monolithic; one long scenario covers every event, log routing, state routing, and unknown-event behavior, which makes future failures harder to localize.
- `SceneListChanged` state handling remains weak: `StateStore::apply_event` returns `true` and broadcasts, but it does not refresh or update the scene list and does not update `updated_at`. The new test only checks that a state broadcast arrives with the old current scene, so stale scene-list data would pass.
- `InputCreated` is treated as an audio input without using OBS input kind/type data. If OBS emits `InputCreated` for a non-audio source, the server can add it to `audio_inputs` incorrectly.
- Unknown-event drop coverage uses a subsequent known event as the synchronization barrier. That is a reasonable deterministic improvement over sleeps, but the fake OBS/server path still lacks an explicit "event processed" acknowledgment.
- Several older server and IPC integration helpers still use fixed sleeps for readiness and shutdown, and background tasks are still not joined or aborted deterministically.
- The public event payload still intentionally lacks timestamps, raw OBS event names, mutation reasons, and stable event IDs. That is acceptable only if the current narrow contract is treated as deliberate compatibility surface.
- Log broadcasting is still manual and sparse. Ordinary `tracing` records are not bridged to IPC, so the TUI log panel is not yet the full server log stream.
- `reload_config` updates command execution config and snapshot metadata, but changed OBS host/password/reconnect settings still need reconnect-path coverage. `dump-config` still does not reuse the same reload-and-rebroadcast path after writing.
- OBS supervisor still does not detect passive OBS disconnects by itself; it waits for shutdown or explicit reconnect while stale `ObsClient` handles can remain available until a later request fails.

Regressions or compatibility risks:

- The `events` topic is now a real public compatibility contract. Any payload shape change, dropped/added variant, or timestamp/reason addition needs tests and README changes in the same iteration.
- The current state path can broadcast stale data for OBS scene-list mutations, making `state` subscribers believe the cache is fresh when it has not been refreshed.
- The current input event model can conflate all OBS inputs with audio inputs, which risks incorrect TUI/CLI state after non-audio source creation/removal.
- Import-boundary tests are stronger than before but can still miss fully-qualified references outside import lists, so they should not be treated as complete architecture enforcement yet.
- Unknown OBS events are deliberately not observable to clients. This keeps the public contract small but means clients cannot inspect vendor or newly introduced OBS events until the project adds explicit variants.
- Background test tasks and fake IPC/OBS servers still lack deterministic shutdown/join handles in several helpers, which can hide races or leak tasks across tests.
- The repository still contains orchestration/planning artifacts whose ownership is unclear, including lowercase `plan.md` and JSONL telemetry files.

## Next Iteration Priorities

### P0: Make Event State Semantics Honest

1. Refresh state after scene-list mutations.
   - When `ObsEvent::SceneListChanged` arrives, have `ObsSupervisor` fetch a full snapshot from OBS before broadcasting state, or introduce an explicit stale/refresh-pending state if refresh fails.
   - Ensure `updated_at` changes only when the snapshot is actually refreshed or intentionally marked stale.
   - Add fake-OBS-to-IPC coverage proving a created/removed/renamed scene is reflected in the `state` snapshot, not just in the `events` topic.

2. Correct input-created/input-removed state semantics.
   - Use OBS input kind/type data or a full input-list refresh before adding new entries to `audio_inputs`.
   - Decide whether the public `InputCreated`/`InputRemoved` payloads are all OBS inputs or audio-only inputs; update README and tests accordingly.
   - Add coverage for a non-audio input creation so it does not silently appear as an audio input.

3. Split and strengthen event routing integration tests.
   - Separate positive event payload tests from state-cache tests and unknown-event negative tests.
   - Keep marker/barrier synchronization, but prefer explicit fake-OBS/server acknowledgments where practical.
   - Assert full state predicates for each event class so stale broadcasts cannot satisfy the test.

### P0: Finish Architecture Guard Hardening

4. Consolidate boundary guard implementation.
   - Move duplicate source-scanning helpers from `tests/architecture_boundaries.rs` and `tests/dependency_boundaries.rs` into a shared test helper.
   - Remove the older literal-string boundary test from `tests/server_integration.rs` once the dedicated boundary tests cover the same rule.
   - Add self-tests for comments, strings, test-only modules, grouped imports, multiline imports, and fully-qualified production references.

5. Catch fully-qualified boundary violations.
   - Extend the guard to scan production source for forbidden paths outside `use` statements, or switch to a Rust parser-based approach if the string scanner keeps growing.
   - Enforce at least these production boundaries: IPC must not reference `crate::obs`/`obsctl_rs::obs`; domain must not reference IPC protocol or OBS client implementation; CLI/TUI must not reference OBS implementation modules.

### P1: Server Runtime Robustness

6. Detect passive OBS disconnects in `ObsSupervisor`.
   - Add an explicit connection-closed signal from the `ObsClient` task to supervisor.
   - Clear `obs_handle`, mark state disconnected, publish a typed log event, and enter reconnect delay without waiting for another command.
   - Add fake OBS server lifecycle coverage proving state changes when OBS closes the socket silently.

7. Harden request-timeout cleanup.
   - Cover multiple concurrent timeouts with late responses arriving out of order.
   - Cover disconnect during an in-flight timeout and prove pending requests complete promptly.
   - Update the fake OBS delayed-response path so one delayed response does not block reads for unrelated requests unless the test explicitly needs that behavior.
   - Avoid blocking `ObsClient::request` on cancellation bookkeeping if the client task is already gone.

8. Complete reload/dump refresh semantics.
   - After `dump-config`, reload config through the same path used by `reload_config_from_disk`.
   - Merge aliases/shortcuts into the current snapshot and rebroadcast state after dump.
   - Add tests for changed scene/audio aliases after dump and changed reconnect settings after reload plus explicit reconnect.

### P1: Logging Completeness

9. Bridge `tracing` to typed IPC log events.
   - Add a bounded, redacting tracing layer or server-local log sink that publishes `LogEvent`.
   - Keep manual high-value lifecycle events only where they add user-facing clarity.
   - Prevent log broadcast loops and avoid blocking tracing on slow IPC subscribers.
   - Add tests proving warnings/errors emitted through tracing reach `logs` subscribers and secrets are redacted.

10. Improve TUI log rendering ergonomics.
   - Truncate or elide long targets/messages so logs do not dominate narrow panels.
   - Consider hiding module targets by default or showing them only in debug mode.
   - Add widget assertions for long target names and mixed severity ordering.

### P2: Test and Repository Hygiene

11. Replace sleep-based test server readiness.
    - Add explicit readiness channels and shutdown handles for fake IPC and server integration helpers.
    - Join or abort background tasks/threads deterministically.
    - Remove duplicate test server setup where a shared helper can stay simple.

12. Clean planning and telemetry files.
    - Remove or intentionally document the untracked lowercase `plan.md`.
    - Decide whether `ALTERNATIVES.jsonl`, `SCORES.jsonl`, and similar orchestration artifacts belong in the repo.
    - Keep `PLAN.md`, `IMPLEMENTATION_CHECK_PLAN.md`, `AGENT_LOG.md`, and `MEMORY.md` roles distinct.

### P3: Product Expansion After Hardening

13. Recording control.
    - Add OBS requests: `StartRecord`, `StopRecord`, `PauseRecord`, `ResumeRecord`, `GetRecordStatus`.
    - Add CLI commands: `obsctl record start|stop|pause|resume|status`.
    - Add IPC command names and TUI header recording indicator.

14. Streaming control.
    - Add OBS requests: `StartStream`, `StopStream`, `GetStreamStatus`.
    - Add CLI commands and TUI status indicator.

15. Scene transition support.
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
- Cross-layer adapters should live in server/application modules or use pure domain types; the domain layer should not become a dumping ground for IPC-to-OBS coupling.
- Public CLI/IPC behavior is a compatibility surface: changes to error codes, JSON envelopes, or topic payload shapes need docs and representative tests in the same iteration.
