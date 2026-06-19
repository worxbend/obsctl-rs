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

Latest verification:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features` with 207 tests passing

## Review Findings From Latest Iteration

Fully implemented:

- `PublicErrorCode` remains the audited public IPC taxonomy and now documents why IPC owns daemon-reachable wire codes while `ObsctlError::exit_code()` owns local process failures.
- Every current `ObsctlError` variant is covered by tests for both public IPC code mapping and local process exit-code intent.
- `ErrorPayload::new` and `ErrorPayload::from_code` redact messages at the IPC error-payload boundary.
- Non-JSON CLI proxy error output now redacts with the same `redacted_message` utility used by `--json`.
- CLI fake daemon errors now use `ErrorPayload::from_code`, so unknown daemon-supplied messages exercise the same redaction path.
- Redaction coverage includes config-like key/value strings, JSON-like secret fields, URLs with credentials, bearer tokens, mixed-case sensitive keys, and unknown daemon error messages in both JSON and default CLI modes.
- Representative IPC wire JSON tests now cover command requests, subscribe requests, success responses, all public error codes, state events, OBS event examples, and typed log events.
- README documents the frozen CLI/IPC contracts, including the `INVALID_TOPIC` to `IPC_PROTOCOL_ERROR` compatibility decision.
- `support::json::redact_secrets` now handles mixed-case secret field names.

Partial or incomplete:

- The `events` topic is still not a real end-to-end OBS event stream. The current server applies OBS events to state, but does not publish normalized OBS events to `TOPIC_EVENTS`; the README and protocol unit test now describe an observable surface that integration tests do not prove.
- The OBS event wire example is not clearly tied to the actual internal `ObsEvent` model. README shows raw obs-websocket-style `eventType/eventData`, while the protocol unit test uses normalized `type/scene_name` data.
- Invalid subscription topics are documented as `IPC_PROTOCOL_ERROR`, but the existing integration test still only asserts that subscription failed. It should assert the exact wire code and representative message.
- Wire compatibility tests mostly serialize hand-built JSON values. They lock envelope shape, but do not always prove real domain structs or server paths serialize to the documented shape.
- `support::json::redact_secrets` and `ipc::protocol::redacted_message` are separate redaction implementations with overlapping but different behavior; the structured JSON redactor is currently unused by production code.
- Public error mapping tests rely on a manual variant-count constant. The exhaustive Rust matches are useful, but future maintainers still need a clearer pattern for updating variant fixtures intentionally.
- Fake OBS delayed responses currently sleep inside the connection handler before reading additional messages. That is adequate for a single late-response regression test, but it does not exercise concurrent late responses or timeout behavior under pipelined requests.
- Some server integration helpers still use fixed sleeps for readiness and shutdown.
- Log broadcasting is still manual and sparse. Ordinary `tracing` logs are not automatically bridged to IPC, so the TUI log panel does not yet represent the full server log stream promised by the product contract.
- `reload_config` updates command execution config and snapshot metadata, but changed OBS host/password/reconnect settings still need reconnect-path coverage. `dump-config` still does not reuse the same reload-and-rebroadcast path after writing.
- OBS supervisor still does not detect passive OBS disconnects by itself; it waits for shutdown or explicit reconnect while stale `ObsClient` handles can remain available until a later request fails.

Regressions or compatibility risks:

- `INVALID_TOPIC` is intentionally no longer a wire code for bad subscriptions. This is now documented, but still needs an integration wire assertion so it does not drift.
- The newly documented OBS event example can mislead clients because event broadcasting is not implemented end to end and the documented shape conflicts with the protocol unit fixture.
- Boundary redaction is defense in depth, not a proof that arbitrary formatted messages are safe. Structured fields remain safer than secret-bearing strings.
- Background test tasks and fake IPC servers still lack deterministic shutdown/join handles in several helpers, so integration coverage can leak tasks and hide races.
- The repository still contains orchestration/planning artifacts whose intended ownership is unclear, including lowercase `plan.md` and JSONL telemetry files.

## Next Iteration Priorities

### P0: Finish Public Contract Freeze

1. Make the `events` IPC topic real or remove it from the public contract.
   - Choose one OBS event payload shape and make README, protocol tests, and implementation agree.
   - Prefer a typed normalized event contract derived from `ObsEvent` rather than leaking raw obs-websocket naming.
   - Publish OBS events to `TOPIC_EVENTS` without disrupting state updates.
   - Add end-to-end tests proving an `events` subscriber receives scene/audio OBS events and state-only/log-only subscribers do not.

2. Close remaining wire-path compatibility assertions.
   - Update `invalid_topic_returns_error` to assert `IPC_PROTOCOL_ERROR` and the documented unknown-topic message.
   - Add at least one raw server-session test that reads the newline-delimited JSON response for an invalid subscribe request.
   - Where practical, build state/log/event compatibility fixtures from real domain structs instead of ad hoc `json!` payloads.
   - Keep README examples synchronized with the asserted wire fixtures.

3. Consolidate redaction utilities.
   - Move shared redaction behavior into a support module used by `ErrorPayload`, `LogEvent`, CLI output, and structured JSON redaction.
   - Keep redaction idempotent so repeated boundary sanitization does not corrupt `[REDACTED]` markers.
   - Add tests for Unicode-adjacent secrets, URL-encoded credentials, repeated redaction, and structured `serde_json::Value` payloads.
   - Document redaction limits and prefer structured non-secret fields over scanning formatted messages.

### P1: Server Runtime Robustness

4. Detect passive OBS disconnects in `ObsSupervisor`.
   - Add an explicit connection-closed signal from the `ObsClient` task to supervisor.
   - Clear `obs_handle`, mark state disconnected, publish a typed log event, and enter reconnect delay without waiting for another command.
   - Add fake OBS server lifecycle coverage proving state changes when OBS closes the socket silently.

5. Harden request-timeout cleanup.
   - Cover multiple concurrent timeouts with late responses arriving out of order.
   - Cover disconnect during an in-flight timeout and prove pending requests complete promptly.
   - Update the fake OBS delayed-response path so one delayed response does not block reads for unrelated requests unless the test explicitly needs that behavior.
   - Avoid blocking `ObsClient::request` on cancellation bookkeeping if the client task is already gone.

6. Complete reload/dump refresh semantics.
   - After `dump-config`, reload config through the same path used by `reload_config_from_disk`.
   - Merge aliases/shortcuts into the current snapshot and rebroadcast state after dump.
   - Add tests for changed scene/audio aliases after dump and changed reconnect settings after reload plus explicit reconnect.

### P1: Logging Completeness

7. Bridge `tracing` to typed IPC log events.
   - Add a bounded, redacting tracing layer or server-local log sink that publishes `LogEvent`.
   - Keep manual high-value lifecycle events only where they add user-facing clarity.
   - Prevent log broadcast loops and avoid blocking tracing on slow IPC subscribers.
   - Add tests proving warnings/errors emitted through tracing reach `logs` subscribers and secrets are redacted.

8. Improve TUI log rendering ergonomics.
   - Truncate or elide long targets/messages so logs do not dominate narrow panels.
   - Consider hiding module targets by default or showing them only in debug mode.
   - Add widget assertions for long target names and mixed severity ordering.

### P2: Test and Repository Hygiene

9. Replace sleep-based test server readiness.
    - Add explicit readiness channels and shutdown handles for fake IPC and server integration helpers.
    - Join or abort background tasks/threads deterministically.
    - Remove duplicate test server setup where a shared helper can stay simple.

10. Clean planning and telemetry files.
    - Remove or intentionally document the untracked lowercase `plan.md`.
    - Decide whether `ALTERNATIVES.jsonl`, `SCORES.jsonl`, and similar orchestration artifacts belong in the repo.
    - Keep `PLAN.md`, `IMPLEMENTATION_CHECK_PLAN.md`, `AGENT_LOG.md`, and `MEMORY.md` roles distinct.

### P3: Product Expansion After Hardening

11. Recording control.
    - Add OBS requests: `StartRecord`, `StopRecord`, `PauseRecord`, `ResumeRecord`, `GetRecordStatus`.
    - Add CLI commands: `obsctl record start|stop|pause|resume|status`.
    - Add IPC command names and TUI header recording indicator.

12. Streaming control.
    - Add OBS requests: `StartStream`, `StopStream`, `GetStreamStatus`.
    - Add CLI commands and TUI status indicator.

13. Scene transition support.
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
