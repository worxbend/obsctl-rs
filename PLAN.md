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
- CLI local/proxy commands, server-unavailable UX, and a first-pass `--json` flag.
- Ratatui TUI model/session/widgets with typed state and typed server log rendering.
- Typed IPC `LogEvent` contract with `level`, `message`, optional `target`, and RFC3339 `timestamp`.
- Explicit server-side typed log publication for selected daemon, supervisor, and command-executor events.

Latest verification:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features` with 181 tests passing

## Review Findings From Latest Iteration

Fully implemented:

- `LogLevel` and `LogEvent` are defined in `src/ipc/protocol.rs` and serialize under the existing generic `{"type":"event","topic":"logs","data":...}` envelope.
- `BroadcastHub::publish_log` routes typed log events only to `logs` subscribers.
- Server daemon, supervisor, and command executor publish several typed lifecycle/config events.
- TUI model stores structured log entries and the logs widget colors by severity instead of parsing strings.
- IPC and TUI tests cover typed log subscription, topic isolation, malformed typed log payloads, and rendering.
- `reload_config` now loads from disk, validates, updates in-memory config, merges aliases/shortcuts into the current snapshot, and rebroadcasts state.
- OBS requests now time out using `connection.request_timeout_ms`, with cleanup of timed-out pending requests.

Partial or incomplete:

- Log broadcasting is still manual and sparse. Ordinary `tracing` logs are not automatically bridged to IPC, so the TUI log panel does not yet represent the server log stream promised by the product contract.
- CLI `--json` is useful but not a stable scripting contract. It prints bare result objects on success, bare error payloads on server errors, and still prints human-readable text for local connection failures.
- `reload_config` updates command execution config and snapshot metadata, but it does not explicitly prove all post-reload behavior: changed OBS host/password/reconnect settings require reconnect-path coverage, and dump-config still does not refresh/broadcast snapshot metadata after writing.
- OBS supervisor still does not detect passive OBS disconnects by itself; it waits for shutdown or explicit reconnect while stale `ObsClient` handles can remain available until a later request fails.

Regressions or bugs:

- `CONFIG_INVALID` returned through an IPC proxy command maps to CLI exit code `1` instead of the project contract's config exit code `2`.
- `RequestTimeout` is collapsed to IPC error code `OBS_UNAVAILABLE` in the server, while CLI code also knows about `REQUEST_TIMEOUT`. The taxonomy is inconsistent and loses useful failure detail.
- Background-thread fake IPC helpers in CLI tests use sleeps for readiness and do not expose deterministic shutdown, which can become flaky as integration coverage grows.
- The repository contains an untracked lowercase `plan.md` alongside canonical `PLAN.md`; this should be removed or intentionally adopted in a cleanup pass to avoid split planning sources.

## Next Iteration Priorities

### P0: Contract Correctness

1. Fix CLI proxy exit-code mapping.
   - Map `CONFIG_INVALID` to exit `2`.
   - Add CLI integration coverage for proxy `reload-config` returning `CONFIG_INVALID`.
   - Audit all IPC error codes against `domain::errors::ObsctlError::exit_code`.

2. Normalize IPC timeout/error taxonomy.
   - Decide whether server should expose request timeout as `REQUEST_TIMEOUT` or keep it under `OBS_UNAVAILABLE`.
   - Make `CommandExecutor::error_code`, CLI mapping, tests, and README agree.
   - Preserve actionable user messages without exposing connection secrets.

3. Stabilize `--json` output.
   - Define one JSON envelope for proxy success and failure, including local server-unavailable failures.
   - Keep stdout machine-readable and stderr reserved for human diagnostics only when `--json` is not set.
   - Cover `status`, `obs-status`, command success, server error, config error, and server-unavailable paths.

### P1: Server Runtime Robustness

4. Detect passive OBS disconnects in `ObsSupervisor`.
   - Add an explicit connection-closed signal from `ObsClient` task to supervisor.
   - Clear `obs_handle`, mark state disconnected, publish a typed log event, and enter reconnect delay without waiting for another request.
   - Add fake OBS integration coverage for server lifecycle, not only raw client behavior.

5. Harden request-timeout cleanup.
   - Cover late OBS responses after timeout and prove they do not poison future requests.
   - Cover multiple concurrent timeouts and disconnect during timeout.
   - Avoid blocking `ObsClient::request` on cancellation bookkeeping if the client task is already gone.

6. Complete reload/dump refresh semantics.
   - After `dump-config`, reload config through the same path used by `reload_config_from_disk`.
   - Merge aliases/shortcuts into snapshot and rebroadcast state after dump.
   - Add tests for changed scene/audio aliases after dump and changed reconnect settings after reload plus explicit reconnect.

### P1: Logging Completeness

7. Bridge `tracing` to typed IPC log events.
   - Add a bounded, redacting tracing layer or server-local log sink that publishes `LogEvent`.
   - Keep manual high-value lifecycle events only where they add user-facing clarity.
   - Prevent log broadcast loops and avoid blocking tracing on slow IPC subscribers.
   - Add tests proving warnings/errors emitted through tracing reach `logs` subscribers and secrets are redacted.

8. Consolidate redaction utilities.
   - Reuse or align `support::json` redaction and `ipc::protocol::redacted_message`.
   - Expand tests for JSON-like strings, URLs with credentials, bearer tokens, and mixed-case sensitive keys.
   - Document redaction limits: structured fields are safer than arbitrary formatted messages.

9. Improve TUI log rendering ergonomics.
   - Truncate or elide long targets/messages so logs do not dominate narrow panels.
   - Consider hiding module targets by default or showing them only in debug mode.
   - Add widget assertions for long target names and mixed severity ordering.

### P2: Test and Repository Hygiene

10. Replace sleep-based test server readiness.
    - Add explicit readiness channels and shutdown handles for fake IPC helpers.
    - Join or abort background tasks/threads deterministically.
    - Remove duplicate test server helpers where a shared helper can stay simple.

11. Clean planning and telemetry files.
    - Remove or intentionally document the untracked lowercase `plan.md`.
    - Decide whether `ALTERNATIVES.jsonl` and similar orchestration artifacts belong in the repo.
    - Keep `PLAN.md`, `IMPLEMENTATION_CHECK_PLAN.md`, `AGENT_LOG.md`, and `MEMORY.md` roles distinct.

12. Add protocol compatibility tests.
    - Snapshot representative IPC wire JSON for command, response, state event, OBS event, and typed log event.
    - Include a migration note that raw string log payloads are no longer accepted by the TUI unless backward compatibility is intentionally restored.

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
