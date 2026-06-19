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
- `cargo test --all-targets --all-features` with 192 tests passing

## Review Findings From Latest Iteration

Fully implemented:

- `PublicErrorCode` now defines the public IPC error taxonomy and CLI exit-code mapping in `src/ipc/protocol.rs`.
- `CommandExecutor` now converts `ObsctlError::RequestTimeout` to `REQUEST_TIMEOUT` instead of collapsing it into `OBS_UNAVAILABLE`.
- `CONFIG_INVALID` returned by the daemon now maps to CLI exit code `2`, including proxy `reload-config` coverage.
- `SHUTDOWN_DISABLED` is represented as a typed domain error and public IPC error code rather than an encoded `ObsRequestFailed` string.
- Proxy `--json` output now uses one envelope for success, daemon errors, protocol errors, and local server-unavailable failures:
  `{"ok":bool,"result":...,"error":...,"exit_code":number}`.
- `--json` proxy failures keep stderr empty and emit machine-readable stdout for server-unavailable, config, OBS, and command errors.
- README documents the observable CLI contract, proxy command set, JSON envelope, IPC error codes, and timeout taxonomy.
- Tests cover status, obs-status, scene/mute/volume success envelopes, daemon error envelopes, config error exit code `2`, local server-unavailable JSON, server timeout IPC code, and late OBS responses after timeout.
- One CLI fake IPC helper now uses a readiness channel instead of a fixed bind sleep.

Partial or incomplete:

- The public error contract is centralized for IPC/CLI proxy behavior, but `ObsctlError::exit_code()` still exists separately. The two mappings intentionally differ for some execution contexts, but that split is not documented in code and should be made explicit or collapsed into a single authority.
- Error messages are redacted for `--json` CLI envelopes, but `ErrorPayload::new` and non-JSON CLI error printing still rely on upstream callers to provide secret-safe messages. Redaction belongs at the IPC error-payload boundary as defense in depth.
- Invalid subscription topics now surface as `IPC_PROTOCOL_ERROR`; previous tests only asserted that subscription failed. This may be acceptable, but the compatibility impact should be locked with a wire-format test or intentionally documented.
- Fake OBS delayed responses currently sleep inside the connection handler before reading additional messages. That is adequate for a single late-response regression test, but it does not exercise concurrent late responses or timeout behavior under pipelined requests.
- Some server integration helpers still use fixed sleeps for readiness and shutdown.
- Log broadcasting is still manual and sparse. Ordinary `tracing` logs are not automatically bridged to IPC, so the TUI log panel does not yet represent the full server log stream promised by the product contract.
- `reload_config` updates command execution config and snapshot metadata, but changed OBS host/password/reconnect settings still need reconnect-path coverage. `dump-config` still does not reuse the same reload-and-rebroadcast path after writing.
- OBS supervisor still does not detect passive OBS disconnects by itself; it waits for shutdown or explicit reconnect while stale `ObsClient` handles can remain available until a later request fails.

Regressions or compatibility risks:

- `INVALID_TOPIC` is no longer a wire code for bad subscriptions. If any client depended on it, this is a breaking protocol change.
- Unknown daemon error codes still fall back to exit code `1`; that is safe, but compatibility tests should prove known codes are exhaustive and docs stay synchronized.
- Background test tasks and fake IPC servers still lack deterministic shutdown/join handles in several helpers, so integration coverage can leak tasks and hide races.
- The repository still contains orchestration/planning artifacts whose intended ownership is unclear, including lowercase `plan.md` and JSONL telemetry files.

## Next Iteration Priorities

### P0: Contract Hardening

1. Make public error mapping a single audited contract.
   - Decide whether `PublicErrorCode` should move out of `ipc::protocol` into a domain-facing module, or document why IPC owns the public wire taxonomy.
   - Add a test that every `ObsctlError` variant has both a public IPC code and an intended process exit class.
   - Document the difference between local process exit mapping and daemon-reachable IPC error mapping where they intentionally differ.

2. Redact IPC error payloads at the boundary.
   - Apply `redacted_message` inside `ErrorPayload::new` or an equivalent constructor used by all server-generated errors.
   - Replace direct `ErrorPayload { code, message }` construction in tests/helpers where practical so tests exercise the real boundary.
   - Ensure non-JSON CLI error output is redacted the same way as `--json`.
   - Add tests for config-like strings, URLs with credentials, bearer tokens, mixed-case sensitive keys, and unknown daemon-supplied messages.

3. Lock public wire compatibility with snapshots.
   - Add representative wire JSON tests for command request, subscribe request, success response, each public error code, state event, OBS event, and typed log event.
   - Decide and document whether bad subscription topics should return `IPC_PROTOCOL_ERROR` or restore a distinct `INVALID_TOPIC` public code.
   - Add README or migration notes for any intentionally removed/renamed wire codes.

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

8. Consolidate redaction utilities.
   - Reuse or align `support::json` redaction and `ipc::protocol::redacted_message`.
   - Document redaction limits: structured fields are safer than arbitrary formatted messages.
   - Prefer structured error/log fields for sensitive data instead of relying on string scanning.

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

12. Add protocol compatibility tests for typed logs.
    - Snapshot raw typed log event JSON and TUI ingestion behavior.
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
- Public CLI/IPC behavior is a compatibility surface: changes to error codes, JSON envelopes, or topic payload shapes need docs and representative tests in the same iteration.
