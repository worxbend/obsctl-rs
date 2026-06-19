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
- `cargo test --all-targets --all-features` with 220 tests passing

## Review Findings From Latest Iteration

Fully implemented:

- `ipc::protocol` no longer imports `obs::client::ObsEvent`; the public IPC wire module now owns only `ObsEventPayload`, `ServerMessage`, log events, and error taxonomy.
- OBS-to-public-event conversion was moved out of `ipc::protocol` into `domain::events::normalize_obs_event`, and `ObsSupervisor` now publishes only normalized `ObsEventPayload` values.
- `BroadcastHub::publish_obs_event` now accepts already-normalized public event payloads instead of OBS-client events.
- Public wire fixtures now cover every current `ObsEventPayload` variant: `CurrentProgramSceneChanged`, `SceneListChanged`, `InputCreated`, `InputRemoved`, `InputMuteStateChanged`, and `InputVolumeChanged`.
- The state event fixture is now built from a real `ObsSnapshot`, reducing one ad hoc fixture gap.
- The server-path OBS event routing test no longer assumes the next state event is the OBS-triggered update; it loops until the expected state snapshot is observed.
- README now documents that `events` publishes only known normalized scene/audio events, that unknown/new OBS events are dropped, that scene-list mutations collapse to `SceneListChanged`, and that timestamps/raw event names/stable IDs are absent.
- Verification passes: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets --all-features`.

Partial or incomplete:

- The conversion leak was moved, not fully solved. `domain::events` imports both `ipc::protocol::ObsEventPayload` and `obs::client::ObsEvent`, so the nominal domain layer is acting as a cross-layer adapter between OBS internals and public IPC wire types.
- The new dependency guard checks `src/ipc/protocol.rs`, `src/cli`, and `src/tui` for the literal string `obs::client`, but it does not protect `src/domain`, `src/ipc` broadly, or grouped imports such as `crate::obs::{client::ObsEvent}`.
- Server-path event coverage still exercises only `CurrentProgramSceneChanged` and `InputMuteStateChanged`. `SceneListChanged`, `InputCreated`, `InputRemoved`, `InputVolumeChanged`, and unknown-event drops are covered by unit/wire tests but not by the real fake-OBS-to-IPC path.
- The logs-subscriber negative assertion uses a 150ms quiet window. It is better than checking only one message, but it remains time-based and can miss a delayed misroute.
- Several older server and IPC integration helpers still use fixed sleeps for readiness and shutdown. This iteration replaced one supervisor setup sleep with a polling helper, but the broader test harness is still race-prone.
- The public event payload still intentionally lacks timestamps, raw OBS event names, mutation reasons, and stable event IDs. That is acceptable only if the current narrow contract is treated as deliberate compatibility surface.
- Log broadcasting is still manual and sparse. Ordinary `tracing` records are not bridged to IPC, so the TUI log panel is not yet the full server log stream.
- `reload_config` updates command execution config and snapshot metadata, but changed OBS host/password/reconnect settings still need reconnect-path coverage. `dump-config` still does not reuse the same reload-and-rebroadcast path after writing.
- OBS supervisor still does not detect passive OBS disconnects by itself; it waits for shutdown or explicit reconnect while stale `ObsClient` handles can remain available until a later request fails.

Regressions or compatibility risks:

- `domain::events` now depends on public IPC payloads and OBS-client internals. That dependency direction will make future OBS-client refactors or IPC payload changes easier to accidentally couple.
- The import-boundary test is too narrow to enforce the architecture rule it names, so future violations can pass CI.
- The `events` topic is now a real public compatibility contract. Any payload shape change, dropped/added variant, or timestamp/reason addition needs tests and README changes in the same iteration.
- Unknown OBS events are deliberately not observable to clients. This keeps the public contract small but means clients cannot inspect vendor or newly introduced OBS events until the project adds explicit variants.
- Background test tasks and fake IPC/OBS servers still lack deterministic shutdown/join handles in several helpers, which can hide races or leak tasks across tests.
- The repository still contains orchestration/planning artifacts whose ownership is unclear, including lowercase `plan.md` and JSONL telemetry files.

## Next Iteration Priorities

### P0: Finish The Event Boundary

1. Move OBS event conversion to an explicit server/application adapter or introduce a pure domain event.
   - Do not leave `domain::events` importing both `ipc::protocol` and `obs::client`.
   - Prefer `server::obs_event_adapter` for the direct `ObsEvent -> ObsEventPayload` mapping, or define a pure domain event type that does not depend on IPC or OBS implementation modules.
   - Keep `ObsEventPayload` as the public wire type and make `ObsSupervisor` own the conversion boundary.

2. Strengthen dependency-boundary tests.
   - Assert `src/ipc/protocol.rs` and public IPC modules do not import `crate::obs`, `obsctl_rs::obs`, or grouped `obs::{...}` paths.
   - Assert `src/domain` does not import IPC protocol wire types or OBS client implementation types unless the project intentionally documents a domain-adapter exception.
   - Keep CLI/TUI thin-client checks and make them robust against grouped imports.

3. Expand server-path OBS event coverage.
   - Use fake OBS broadcasts to prove `SceneListChanged`, `InputCreated`, `InputRemoved`, and `InputVolumeChanged` reach `events` subscribers with documented normalized shapes.
   - Prove unknown OBS events do not reach `events`, `state`, or `logs` subscribers beyond expected snapshots/logs.
   - Keep tests matching expected state snapshots by predicate rather than broadcast order.

4. Remove remaining time-based assertions from the event routing test.
   - Replace the logs quiet-window check with deterministic receiver draining or explicit per-topic assertions.
   - Where negative assertions are unavoidable, use bounded deterministic synchronization from the fake OBS/server path instead of arbitrary sleep windows.

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
- Cross-layer adapters should live in server/application modules or use pure domain types; the domain layer should not become a dumping ground for IPC-to-OBS coupling.
- Public CLI/IPC behavior is a compatibility surface: changes to error codes, JSON envelopes, or topic payload shapes need docs and representative tests in the same iteration.
