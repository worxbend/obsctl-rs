# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings   # warnings are errors
cargo test --all-targets --all-features
cargo build --release                                      # binary: target/release/obsctl
```

Running specific tests:

```sh
cargo test --test cli_integration                  # one integration test file
cargo test --test server_integration name_of_test  # one test in that file
cargo test --lib volume::                          # unit tests in a module
cargo test -- --nocapture                          # show println!/tracing output
```

There is no `rust-toolchain` pin; the crate is Rust **edition 2024** and expects a recent stable
toolchain (uses let-chains, `is_some_and`, etc.). No CI workflow runs tests — `release.yml` only
builds tagged releases, so the checks above are the only gate and must be run locally.

Tests never require a real OBS or a real daemon: `tests/support/fake_obs_server.rs` implements the
obs-websocket 5.x handshake over a random local port, and IPC tests bind sockets in `tempfile` dirs.
The support module is pulled in per-file via an inline `mod support { ... }` block (see
`tests/server_integration.rs`), not via a shared crate.

## Architecture

```
OBS Studio <── obs-websocket 5.x ──> obsctl server <── Unix socket IPC ──> obsctl tui
                                                    <── Unix socket IPC ──> obsctl CLI
```

**Hard rule: only `obsctl server` connects to OBS.** The CLI and TUI are thin IPC clients — they
never open a WebSocket, never auto-start the daemon, and get all state pushed to them. This is
enforced by tests, not convention (see *Enforced boundaries*).

Crate layout — package `obsctl-rs`, lib crate `obsctl_rs`, binary `obsctl`. `main.rs` → `lib::run()`
→ `cli::router::run()`, which is the single dispatch point for every mode.

| Module | Role |
|---|---|
| `cli` | `args` (clap derive), `router` (mode dispatch + logger init), `client_commands` (proxy commands → IPC → exit code) |
| `ipc` | Public wire contract: `protocol` (message enums, `PublicErrorCode`, `Topic`), `codec` (newline-framed JSON), `unix_server`/`unix_client`, `session` (`BroadcastHub`, `CommandDispatch`), `socket_path` |
| `server` | Daemon internals: `daemon` (startup/socket lifecycle), `command_executor` (IPC command → OBS request), `obs_supervisor` (connect/reconnect loop), `state_store` (cached `ObsSnapshot`, broadcasts), `client_registry`, `obs_event_adapter` (OBS events → IPC payloads) |
| `obs` | obs-websocket client: `connection`, `auth`, `client` (implementation — *restricted*), `requests` (typed builders), `state` (`ObsSnapshot`, shared with clients), `protocol` |
| `domain` | Pure logic: `command`, `parser` (TUI/palette command strings), `aliases` (alias/shortcut resolution), `volume`, `errors` (`ObsctlError`), `result` |
| `tui` | `app` (event loop), `session` (subscribes to `state`/`events`/`logs`), `event_applier` (`ServerMessage` → `TuiModel`), `model`, `widgets/*`, `theme`, `layout`, `anim`, `completion`, `input` |
| `config` | `model`/`schema` (validation, env-var expansion), `loader`, `writer`, `dump`, `paths` |
| `support` | `validation` (token/env-var hardening), `redaction`, `fs`, `json` |
| `runtime` | `logger` (separate CLI vs server init), `reconnect_policy`, `shutdown` |
| `service` | systemd `--user` unit install/management |

### Two data paths

**Command (request/response):** CLI/TUI opens a short-lived connection → `ClientMessage::Command`
with a correlation `id` → `IpcServer` → `CommandDispatch` → `CommandExecutor` (string-keyed match on
command name, ~`src/server/command_executor.rs:578`) → `obs::requests` → `ServerMessage::Response`
matched back by `id`.

**Event (push):** `ObsSupervisor` receives OBS events → `obs_event_adapter` → `StateStore` mutates
the cached snapshot **and** rebroadcasts → `BroadcastHub` fans out per topic (`state`, `events`,
`logs`) → `TuiEventSession` → `event_applier` → `TuiModel` → redraw. Subscribing pushes an initial
snapshot before any change events; tests must account for it.

`ObsSnapshot` (in `obs::state`) is the shared state shape — it crosses the IPC boundary as the
`state` topic payload, so changing it is a wire-protocol change.

## Enforced boundaries

`tests/architecture_boundaries.rs` and `tests/dependency_boundaries.rs` parse `src/` (comments and
`#[cfg(test)]` modules stripped) and fail the build on violations:

- `src/cli` and `src/tui` must not import `obs::client` — they may use `obs::state` types.
- `src/ipc` must not import **any** `obs::*` implementation module.
- `src/domain` must not import `ipc::protocol` wire types or `obs::client` types.

These guards match `use` statements only; fully-qualified inline paths (`crate::obs::client::X::y()`)
slip past them, so keep the rule in mind rather than trusting the test alone.

## Conventions

**Public contracts change as one unit.** Adding or changing an error variant, IPC command, or error
code means touching all of: `domain::errors::ObsctlError`, `ipc::protocol::PublicErrorCode`, the CLI
exit-code mapping in `cli::client_commands`, the `--json` envelope, the README tables (Exit Codes /
IPC Error Codes / IPC Protocol), and integration tests — in the same change. `ObsctlError` and
`PublicErrorCode` each have an `OBSCTL_ERROR_VARIANT_COUNT` assertion (`src/domain/errors.rs:125`,
`src/ipc/protocol.rs:1067`) that fails when a variant is added without updating the mapping tables.

**Two exit-code mappings, deliberately.** Local failures (init, validate-config, server startup,
socket setup) use the local `ObsctlError` classification. Proxy commands that got a daemon response
use `PublicErrorCode::exit_code()` — that table is the stable daemon-reachable contract.

**Redaction at the boundary, not the presentation layer.** Use `support::redaction` (`redact_message`
/ `redact_json_value`, keyed on `authentication`/`password`/`token`/`auth`) when building log lines
and IPC payloads, not just when printing. Redaction is idempotent by design; keep it that way.

**Untrusted input is validated in `support::validation`.** Env vars, target names, and paths go
through the token helpers (length caps, control-char rejection). `OBSCTL_CONFIG` and socket paths
additionally reject relative paths and symlinks and fall back to defaults rather than erroring.

**No user-facing string literals.** Every message shown to a user goes through `rust_i18n::t!`.
`locales/en.yml` is embedded at compile time as the fallback; `contrib/locales/uk.yml` ships as a
runtime override loaded from `<config_dir>/locales/<locale>.yml` by `localization::FileBackend`. Add
new keys to `locales/en.yml` and keep `localization::SUPPORTED_LOCALES` in sync.

**Test fixtures must come from the real server path.** Hand-written protocol fixtures have
documented APIs the daemon does not actually emit; assert against output produced by the server or
the fake OBS server. Likewise, fake servers expose explicit readiness/shutdown handles — do not add
sleep-based readiness to test helpers.

**Broadcasting without mutating state is a false positive.** When an event should change state,
assert the changed snapshot content, not merely that a broadcast arrived.

TUI widgets are tested by rendering into Ratatui's `TestBackend` at fixed sizes and asserting on the
buffer text (`tests/tui_widget_rendering.rs`); every widget needs coverage for connected,
disconnected, empty, and error states, and the model supports an ASCII/no-icon mode
(`TuiModel::symbol`, `rich_ui`) that rendering must respect.

## Repo docs

`README.md` is the user-facing contract and is kept in sync with behavior — treat its tables as
normative. `MEMORY.md` holds one-line pattern / anti-pattern / learning notes carried over from an
earlier autonomous build loop and is the source of several conventions above.
