# obsctl-rs Implementation Check Plan

This is the strict build and review checklist for implementing `obsctl-rs` from scratch in Rust with Ratatui. It adapts the existing `obsctl-cr` implementation requirements to the new Rust stack.

## Project Contract

- Name: `obsctl-rs`.
- Language: Rust.
- TUI framework: Ratatui with Crossterm.
- Primary goal: local OBS Studio control daemon with CLI and TUI clients.
- Target OBS protocol: obs-websocket 5.x.

Required architecture:

```text
OBS Studio <---- obs-websocket 5.x ----> obsctl server <---- Unix socket IPC ----> obsctl TUI
                                                               <---- Unix socket IPC ----> obsctl CLI
```

Hard rules:

- `obsctl server` is the only normal-mode process that connects directly to OBS.
- CLI commands must be thin IPC clients, except `init`, `validate-config`, `server`, and `service`.
- TUI must be a thin IPC client and renderer.
- No local TCP listener by default.
- No secret values in logs, errors, TUI panels, or config validation warnings.

## Required Runtime Modes

- `obsctl init`
- `obsctl validate-config`
- `obsctl server`
- `obsctl server --headless`
- `obsctl`
- `obsctl tui`
- `obsctl status`
- `obsctl obs-status`
- `obsctl server-status`
- `obsctl reconnect`
- `obsctl shutdown-server`
- `obsctl scene <target>`
- `obsctl mute <target>`
- `obsctl unmute <target>`
- `obsctl toggle-mute <target>`
- `obsctl vol <target> <0-100>`
- `obsctl volume <target> <0-100>`
- `obsctl dump-config`
- `obsctl reload-config`
- `obsctl service install`
- `obsctl service uninstall`
- `obsctl service status`
- `obsctl service start`
- `obsctl service stop`
- `obsctl service restart`

## Required Rust Stack

Check that the selected crates are present or intentionally replaced:

- `tokio`
- `tokio-tungstenite`
- `futures-util`
- `serde`
- `serde_json`
- `serde_yaml`
- `clap`
- `ratatui`
- `crossterm`
- `thiserror`
- `tracing`
- `tracing-subscriber`
- `tracing-appender`
- `directories` or `xdg`
- `rand`
- `tempfile`
- `assert_cmd`
- `predicates`

Optional but useful:

- `insta` for TUI/widget snapshots.
- `proptest` for parser/codec fuzz-style coverage.

## Required Module Layout

Implementation should keep equivalent boundaries:

```text
src/
  main.rs
  lib.rs
  cli/
  config/
  domain/
  ipc/
  obs/
  server/
  tui/
  service/
  runtime/
  support/
tests/
  support/
```

Review checks:

- `main.rs` only wires process-level startup and exit code mapping.
- OBS protocol code is isolated under `obs/`.
- Ratatui widgets do not perform IPC or OBS calls.
- CLI proxy commands do not import `obs::client`.
- TUI modules do not import `obs::client`.
- Server command executor is the only path from IPC command to OBS action.

## Config Checklist

Default path:

- `~/.config/obsctl/config.yml`

Override:

- `OBSCTL_CONFIG`
- `--config PATH`

Required top-level fields:

- `version`
- `server`
- `connection`
- `reconnect`
- `ui`
- `scenes`
- `audio`
- `keymap`

Validation gates:

- Reject unknown top-level keys.
- Reject unsupported config versions.
- Reject blank host.
- Reject invalid port outside `1..=65535`.
- Reject non-positive request/connect timeouts.
- Reject non-positive UI refresh interval.
- Reject blank configured socket path.
- Reject blank configured pid file.
- Reject negative reconnect delays and jitter.
- Reject `reconnect.max_delay_ms < reconnect.initial_delay_ms`.
- Reject `reconnect.multiplier < 1.0`.
- If `connection.password_env` is non-empty, require the env var for validation.
- Warn for plaintext `connection.password`, without printing the secret.
- Accept legacy `connection.reconnect`.
- Write canonical top-level `reconnect`.
- Preserve `server`, `reconnect`, `ui`, and `keymap` during dump-config.

Alias validation gates:

- Reject duplicate scene aliases.
- Reject duplicate scene shortcuts.
- Reject duplicate audio aliases.
- Reject duplicate audio shortcuts.
- During dump-config, reject alias/shortcut collisions with discovered OBS names.

## Command Parser Checklist

Required parser behavior:

- Accept optional leading `/`.
- Preserve quoted names.
- Support escaped quotes.
- Reject unterminated quotes.
- Reject empty command.
- Reject unknown command.
- Reject wrong argument count.
- Validate volume as integer `0..=100`.

Required palette commands:

- `/help`
- `/quit`
- `/exit`
- `/dump-config`
- `/reload-config`
- `/status`
- `/server-status`
- `/obs-status`
- `/validate-config`
- `/reconnect`
- `/shutdown-server`
- `/connect`
- `/disconnect`
- `/set-scene <target>`
- `/scene <target>`
- `/mute <target>`
- `/unmute <target>`
- `/toggle-mute <target>`
- `/vol <target> <0-100>`
- `/volume <target> <0-100>`

Required CLI-to-palette mapping:

- `scene` -> `/scene`
- `mute` -> `/mute`
- `unmute` -> `/unmute`
- `toggle-mute` -> `/toggle-mute`
- `vol|volume` -> `/vol`
- `status` -> `/status`
- `server-status` -> `/server-status`
- `obs-status` -> `/obs-status`
- `validate-config` -> `/validate-config`
- `reconnect` -> `/reconnect`
- `shutdown-server` -> `/shutdown-server`
- `dump-config` -> `/dump-config`
- `reload-config` -> `/reload-config`

## Alias Resolution Checklist

Resolution order must be:

1. Exact shortcut.
2. Exact alias.
3. Exact OBS object name.
4. Case-insensitive alias.
5. Case-insensitive OBS object name.

Required failures:

- Unknown scene returns `SCENE_NOT_FOUND`.
- Unknown audio input returns `AUDIO_INPUT_NOT_FOUND`.
- Ambiguous scene/audio target returns `ALIAS_AMBIGUOUS`.
- Ambiguous commands must not send OBS requests.

## IPC Checklist

Transport:

- Unix domain sockets.
- Newline-delimited JSON.

Socket path:

- Prefer `$XDG_RUNTIME_DIR/obsctl/obsctl.sock`.
- Fallback to `/tmp/obsctl-$UID/obsctl.sock`.
- Honor configured `server.socket_path`.

Request model:

```json
{"id":"req-000001","type":"command","command":{"name":"set_scene","target":"main"}}
```

Subscribe model:

```json
{"id":"req-000002","type":"subscribe","topics":["state","events","logs"]}
```

Success response:

```json
{"id":"req-000001","type":"response","ok":true,"result":{"message":"ok"}}
```

Error response:

```json
{"id":"req-000001","type":"response","ok":false,"error":{"code":"OBS_UNAVAILABLE","message":"OBS is unavailable"}}
```

Pushed event:

```json
{"type":"event","topic":"state","data":{"connected":true}}
```

Supported topics:

- `state`
- `events`
- `logs`

Supported command payload names:

- `ping`
- `get_server_status`
- `get_obs_status`
- `get_snapshot`
- `set_scene`
- `mute`
- `unmute`
- `toggle_mute`
- `set_volume`
- `dump_config`
- `reload_config`
- `validate_config`
- `reconnect_obs`
- `shutdown_server`

Protocol gates:

- Validate request type.
- Validate subscription topics.
- Correlate responses by request ID.
- Do not panic on malformed JSON.
- Do not let one bad client crash the server.
- Immediately send current snapshot to new `state` subscribers.
- Broadcast OBS events only to `events` subscribers.
- Broadcast server logs only to `logs` subscribers.

## OBS WebSocket Checklist

Connection flow:

1. Connect WebSocket to configured host/port.
2. Read Hello opcode `0`.
3. Send Identify opcode `1`.
4. Wait for Identified opcode `2`.
5. Send Request opcode `6` only after Identified.
6. Match RequestResponse opcode `7` by `requestId`.
7. Consume Event opcode `5`.

Authentication formula:

- `secret = base64(sha256(password + salt))`
- `authentication = base64(sha256(secret + challenge))`

Required requests:

- `GetVersion`
- `GetSceneList`
- `GetCurrentProgramScene`
- `SetCurrentProgramScene`
- `GetInputList`
- `GetInputMute`
- `SetInputMute`
- `ToggleInputMute`
- `GetInputVolume`
- `SetInputVolume`

Required client behavior:

- Generate unique request IDs.
- Maintain pending request map.
- Remove pending entries on success, failure, timeout, and disconnect.
- Apply configured request timeout.
- Convert OBS request failures into typed errors.
- Fail all pending requests when socket closes.
- Reset identified/connected state on close.
- Dispatch OBS events to supervisor.
- Do not subscribe to high-volume meter events by default.

Security gates:

- Do not log password.
- Do not log generated authentication string.
- Redact sensitive config fields in debug output.

## Server Checklist

Server startup:

- Resolve config path.
- Load config.
- Validate config.
- Resolve socket path.
- Create runtime directory.
- Probe existing socket.
- Remove stale socket only when no live server responds.
- Start IPC accept loop.
- Start OBS supervisor.
- Attempt OBS connection.
- Authenticate if required.
- Fetch initial snapshot.
- Broadcast initial state.
- Continue serving IPC while OBS is disconnected.

Server shutdown:

- Stop accept loop.
- Close active client sessions.
- Stop OBS supervisor.
- Close OBS WebSocket.
- Remove socket file.
- Flush logger.

Server-owned responsibilities:

- OBS connection.
- OBS reconnect loop.
- Authoritative state cache.
- Command execution.
- Alias resolution.
- Config reload.
- Config dump.
- IPC subscriptions.
- Log broadcasting.

Unavailable OBS behavior:

- `status`, `server-status`, and `obs-status` still return meaningful state.
- Scene/audio commands return `OBS_UNAVAILABLE`.
- TUI shows disconnected state and last error.
- Reconnect loop continues according to config.
- Local IPC remains available.

Shutdown command:

- `shutdown_server` returns `SHUTDOWN_DISABLED` unless `server.allow_remote_shutdown: true`.
- When enabled, response must be sent before shutdown proceeds.

## Reconnect Checklist

Reconnect policy:

- Enabled by default.
- Endless by default in server mode.
- Initial delay default `500ms`.
- Max delay default `10000ms`.
- Multiplier default `1.8`.
- Jitter default `250ms`.
- Reset backoff after successful connection.

Supervisor behavior:

- Publish disconnected snapshot after OBS connection failure.
- Keep old config available for local commands.
- Reconnect without restarting IPC server.
- Refresh full snapshot after reconnect.
- Broadcast state after reconnect.

## State Checklist

Required snapshot fields:

- `connected`
- `obs_studio_version`
- `obs_websocket_version`
- `current_scene`
- `scenes`
- `audio_inputs`
- `last_error`
- `updated_at`

Required scene fields:

- `name`
- `alias`
- `shortcut`
- `group`
- `active`

Required audio fields:

- `name`
- `alias`
- `shortcut`
- `kind`
- `muted`
- `volume_mul`
- `volume_db`
- `volume_percent`

Required server status fields:

- `pid`
- `uptime_seconds`
- `socket_path`
- `client_count`
- `obs_connected`
- `reconnecting`
- `last_error`

State gates:

- Server state is authoritative.
- TUI only renders received state.
- CLI only prints responses/snapshots.
- OBS events update state store.
- State changes broadcast to subscribed clients.

## Dump Config Checklist

Required behavior:

- Execute inside server.
- Use server-owned OBS connection.
- Fetch scene names.
- Fetch audio input names.
- Read existing config from configured path.
- Preserve aliases.
- Preserve shortcuts.
- Preserve groups.
- Preserve stale markers when still stale.
- Preserve top-level daemon settings.
- Preserve reconnect settings.
- Preserve UI and keymap settings.
- Add newly discovered OBS scenes and audio inputs.
- Mark missing configured OBS objects as stale.
- Reject duplicate controls.
- Reject controls that collide with OBS object names.
- Create timestamped backup before overwrite.
- Write atomically.
- Reload server config after write.
- Refresh and broadcast snapshot.

Must not:

- Delete user aliases silently.
- Delete missing OBS resources silently.
- Overwrite without backup.
- Print or log password.

## CLI Checklist

Global options:

- `--config PATH`
- `--log-level debug|info|warn|error`
- `--force`

Proxy command flow:

1. Parse args.
2. Resolve config path and socket path.
3. Connect to local server.
4. On missing server, print startup/service instructions.
5. Exit `3` on missing server.
6. Send IPC request.
7. Wait for correlated response.
8. Print concise output.
9. Return mapped exit code.

Exit codes:

- `0` success.
- `1` generic failure.
- `2` config error.
- `3` server/connection/auth error.
- `4` OBS request error.
- `5` command parse error.
- `6` IPC error.

Review gates:

- Proxy commands do not auto-start server unless this is implemented as an explicit, tested config-controlled feature.
- Proxy commands do not read OBS password.
- Proxy commands do not import OBS WebSocket client.

## Ratatui TUI Checklist

Startup:

- Resolve config.
- Resolve local socket path.
- Connect to daemon.
- Subscribe to `state`, `events`, and `logs`.
- Render dashboard when connected.
- Render server-unavailable screen when connection fails.

Terminal handling:

- Enter alternate screen.
- Enable raw mode.
- Restore terminal on normal exit.
- Restore terminal on command error where possible.
- Handle resize events.
- Tick at configured refresh interval.

Required panels:

- Header.
- Connection status.
- Scenes.
- Scene map.
- Audio.
- Logs.
- Command palette.

Required input:

- `/` opens command palette.
- `:` opens command palette.
- `Backspace` edits palette line.
- `Enter` submits palette command.
- `Esc` cancels palette editing.
- `Ctrl-C` cancels palette editing or exits according to current mode.
- `q` quits dashboard mode.
- `r` sends `/reload-config`.
- `D` sends `/dump-config`.

Rendering gates:

- Handles terminal widths down to a documented minimum.
- Long scene/audio names do not break layout.
- Empty scenes/audio lists render cleanly.
- Disconnected state is visually clear.
- Last command result is visible.
- Recent errors/logs are bounded.
- Widgets do not perform IO.

## Service Checklist

Commands:

- `install`
- `uninstall`
- `status`
- `start`
- `stop`
- `restart`

Install path:

```text
~/.config/systemd/user/obsctl.service
```

Unit file:

```ini
[Unit]
Description=obsctl OBS WebSocket control daemon
After=graphical-session.target
Wants=graphical-session.target

[Service]
Type=simple
ExecStart=<absolute-path-to-obsctl> server --headless
Restart=always
RestartSec=3

[Install]
WantedBy=default.target
```

Review gates:

- No sudo.
- Uses `systemctl --user`.
- Resolves actual executable path.
- Runs `systemctl --user daemon-reload` after install.
- Runs daemon-reload after uninstall.
- Tests use a command runner abstraction, not real systemctl.

## Logging Checklist

Default log path:

- `~/.local/state/obsctl/obsctl.log`

Required behavior:

- Honor `--log-level` in server mode.
- Persist server logs.
- Broadcast compact log events to TUI clients.
- Redact secrets.
- Include request IDs where useful.
- Include state transitions.
- Avoid noisy per-frame TUI logs.

Redaction test cases:

- `connection.password`
- `password_env` resolved value.
- OBS `authentication`.
- Any JSON field named `password`, `authentication`, `auth`, or `token`.

## Test Plan

Unit tests:

- Config load/write/defaults.
- Config validation.
- Config warnings.
- Config dump merge.
- Command parser.
- Alias resolution.
- Volume conversion.
- IPC codec.
- IPC protocol serde.
- OBS auth.
- OBS request serialization.
- Reconnect policy.
- Error to exit-code mapping.
- TUI input controller.
- TUI widget rendering.

Integration tests:

- CLI `init`.
- CLI `validate-config`.
- CLI proxy commands against fake IPC server.
- Unix IPC request/response.
- Unix IPC subscription pushes.
- Server lifecycle with fake OBS.
- OBS client with fake obs-websocket server.
- Reconnect after fake OBS disconnect.
- Dump-config with fake OBS resources.
- Service installer with fake command runner.

Regression tests:

- Quoted scene names.
- Ambiguous alias/shortcut failures.
- Missing server exit code `3`.
- Shutdown disabled by default.
- Unknown top-level config field rejection.
- Plaintext password warning does not include password.
- TUI does not panic on malformed pushed event.
- Pending OBS request map cleans up after timeout.

Manual tests:

- Run server while OBS is closed.
- Open OBS after server starts and observe reconnect.
- Change scene in OBS and verify TUI state update.
- Change scene through CLI and verify OBS/TUI update.
- Mute/unmute through CLI and verify OBS/TUI update.
- Run `dump-config` and inspect backup plus merged config.
- Install and start systemd user service.

## Build Gates

Every implementation milestone must pass:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Before release:

```sh
cargo build --release
```

Documentation gates:

- README documents quick start.
- README documents daemon-first architecture.
- README documents service install.
- README documents config path and password env.
- Command docs match implemented CLI.
- Protocol docs match IPC serde models.

## Review Checklist

Before considering the Rust implementation complete, verify:

- There is exactly one normal-mode OBS WebSocket owner.
- CLI and TUI are IPC clients.
- Server stays alive when OBS is unavailable.
- Server broadcasts state, OBS events, and logs.
- TUI subscribes and renders received state.
- Config validation is strict and secret-safe.
- Dump config preserves user edits and creates backups.
- Service install uses current executable path.
- All user-facing errors are actionable.
- No secrets appear in logs, errors, tests, snapshots, or docs.
- Tests cover both success and failure paths.
