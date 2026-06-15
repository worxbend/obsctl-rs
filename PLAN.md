# obsctl-rs Plan

This plan describes how to build a Rust + Ratatui application equivalent to the existing `obsctl-cr` project. The existing project is a local OBS Studio control tool built around a long-running daemon, thin CLI commands, and a terminal UI. The Rust version should preserve that architecture, protocol, command grammar, configuration semantics, and user-facing behavior while replacing the ANSI renderer with a proper Ratatui application.

## Product Summary

`obsctl-rs` is a local OBS Studio controller for obs-websocket 5.x.

It provides:

- A local daemon that owns the only OBS WebSocket connection.
- A scriptable CLI that sends commands to the daemon through Unix socket IPC.
- A Ratatui dashboard that subscribes to daemon state, OBS events, and server logs.
- Config discovery, validation, safe config dumping, aliases, shortcuts, and service management.
- Robust reconnect behavior so OBS can restart while the local daemon and TUI remain usable.

The core architecture is:

```text
OBS Studio <---- obs-websocket 5.x ----> obsctl server <---- Unix socket IPC ----> obsctl tui
                                                               <---- Unix socket IPC ----> obsctl CLI
```

Hard rule: only `obsctl server` may connect directly to OBS in normal operation. CLI and TUI are local IPC clients.

## Existing Project Functionality

The current Crystal project implements these capabilities:

- `obsctl init` creates `~/.config/obsctl/config.yml`.
- `obsctl validate-config` validates config locally and warns about plaintext passwords.
- `obsctl server` starts the local daemon in foreground mode.
- `obsctl server --headless` starts the same daemon for background/service use.
- `obsctl` and `obsctl tui` start the TUI client.
- `obsctl scene <target>` changes current OBS program scene.
- `obsctl mute <target>`, `unmute`, and `toggle-mute` control audio inputs.
- `obsctl vol|volume <target> <0-100>` sets OBS input volume by percent.
- `obsctl status`, `server-status`, and `obs-status` query daemon and OBS state.
- `obsctl reconnect` asks the daemon to reconnect to OBS.
- `obsctl shutdown-server` is disabled unless `server.allow_remote_shutdown: true`.
- `obsctl dump-config` asks the daemon to fetch OBS scenes/audio and merge them into config.
- `obsctl reload-config` asks the daemon to reload config and rebroadcast state.
- `obsctl service install/start/stop/restart/status/uninstall` manages a `systemd --user` service.

The TUI currently renders:

- Header with app/server/OBS metadata.
- Connection status panel.
- Scene list with active scene, aliases, shortcuts, and groups.
- Scene map grouped by configured group.
- Audio input list with mute and volume status.
- Recent server log lines.
- Command palette.

The TUI command grammar includes:

- `/help`
- `/scene <target>` and `/set-scene <target>`
- `/mute <target>`
- `/unmute <target>`
- `/toggle-mute <target>`
- `/vol <target> <0-100>`
- `/status`
- `/server-status`
- `/obs-status`
- `/validate-config`
- `/reconnect`
- `/connect`
- `/disconnect`
- `/dump-config`
- `/reload-config`
- `/quit`

Quoted names are preserved, for example `/scene "Main Camera"`.

## Rust Implementation Goals

The Rust implementation should improve maintainability and runtime robustness without changing the core product contract.

Primary goals:

- Preserve user-facing commands and config compatibility.
- Use Tokio for async server, IPC, WebSocket, and TUI event orchestration.
- Use Ratatui and Crossterm for a real terminal UI.
- Use typed protocol models with Serde instead of ad hoc JSON handling.
- Keep the daemon/client boundary strict.
- Build a comprehensive test suite before broadening features.

Non-goals for the initial build:

- No TCP remote control API.
- No direct OBS connection from CLI/TUI normal mode.
- No plugin system.
- No OBS browser source/control expansion until core scene/audio behavior is complete.
- No service manager abstraction beyond Linux `systemd --user` initially.

## Recommended Stack

- Language: Rust stable edition 2024 or 2021 if dependency compatibility requires it.
- Async runtime: `tokio`.
- CLI parsing: `clap`.
- TUI: `ratatui` + `crossterm`.
- WebSocket: `tokio-tungstenite`.
- JSON/YAML: `serde`, `serde_json`, `serde_yaml`.
- Config paths: `directories` or `xdg`.
- Errors: `thiserror` for library errors, `anyhow` only at binary boundaries if useful.
- Logging/tracing: `tracing`, `tracing-subscriber`, `tracing-appender`.
- Time: `time` or `chrono`, with one chosen consistently.
- Random jitter: `rand`.
- Temp/test support: `tempfile`, `assert_cmd`, `predicates`, `insta` where snapshots help.

## Workspace Layout

Use a single crate until there is a concrete need to split libraries.

```text
obsctl-rs/
  Cargo.toml
  README.md
  PLAN.md
  IMPLEMENTATION_CHECK_PLAN.md
  src/
    main.rs
    lib.rs
    cli/
      mod.rs
      args.rs
      router.rs
      client_commands.rs
    config/
      mod.rs
      model.rs
      loader.rs
      schema.rs
      writer.rs
      dump.rs
      paths.rs
    domain/
      mod.rs
      command.rs
      parser.rs
      aliases.rs
      errors.rs
      result.rs
      volume.rs
    ipc/
      mod.rs
      socket_path.rs
      codec.rs
      protocol.rs
      unix_client.rs
      unix_server.rs
      session.rs
    obs/
      mod.rs
      auth.rs
      client.rs
      connection.rs
      protocol.rs
      requests.rs
      state.rs
    server/
      mod.rs
      server.rs
      options.rs
      command_executor.rs
      state_store.rs
      client_registry.rs
      obs_supervisor.rs
    tui/
      mod.rs
      app.rs
      model.rs
      session.rs
      event_applier.rs
      input.rs
      layout.rs
      widgets/
        mod.rs
        header.rs
        connection.rs
        scenes.rs
        scene_map.rs
        audio.rs
        logs.rs
        command_palette.rs
    service/
      mod.rs
      systemd_user_service.rs
      installer.rs
    runtime/
      mod.rs
      logger.rs
      reconnect_policy.rs
      shutdown.rs
    support/
      mod.rs
      json.rs
  tests/
    cli_integration.rs
    ipc_integration.rs
    server_integration.rs
    obs_client_integration.rs
    tui_session.rs
    support/
      fake_obs_server.rs
```

## Config Model

Default path: `~/.config/obsctl/config.yml`.

Environment override: `OBSCTL_CONFIG=/path/to/config.yml`.

The Rust version should read the current schema:

```yaml
version: 1
server:
  socket_path:
  pid_file:
  allow_remote_shutdown: false
  start_embedded_if_missing: true
connection:
  host: "127.0.0.1"
  port: 4455
  password_env: "OBS_WEBSOCKET_PASSWORD"
  connect_timeout_ms: 3000
  request_timeout_ms: 2500
reconnect:
  enabled: true
  endless: true
  initial_delay_ms: 500
  max_delay_ms: 10000
  multiplier: 1.8
  jitter_ms: 250
ui:
  refresh_interval_ms: 250
  command_palette_prefix: "/"
  show_icons: true
  theme: "default"
scenes: []
audio:
  inputs: []
keymap:
  quit: ["q", "ctrl+c"]
  command_palette: ["/", ":"]
  reload_config: ["r"]
  dump_config: ["D"]
```

Validation requirements:

- Reject unsupported `version`.
- Reject unknown top-level keys.
- Validate host, port, timeout, UI refresh, reconnect settings, socket path, and pid file.
- Require configured non-empty `password_env` to exist during validation.
- Warn, but do not print the value, when plaintext `connection.password` is present.
- Reject duplicate scene aliases and shortcuts.
- Reject duplicate audio aliases and shortcuts.
- Preserve compatibility with legacy `connection.reconnect`, but write canonical top-level `reconnect`.

## Command Grammar

Implement a shared parser used by CLI mapping and TUI palette input.

Parser requirements:

- Strip optional leading `/`.
- Preserve quoted arguments.
- Support escaped quotes.
- Reject unterminated quotes.
- Validate argument count.
- Validate volume as integer `0..=100`.
- Return typed command enum values.

Alias resolution order:

1. Exact shortcut.
2. Exact alias.
3. Exact OBS name.
4. Case-insensitive alias.
5. Case-insensitive OBS name.

Ambiguous matches must fail without executing OBS requests.

## IPC Protocol

Transport: Unix domain socket with newline-delimited JSON.

Default socket path:

- `$XDG_RUNTIME_DIR/obsctl/obsctl.sock`
- fallback `/tmp/obsctl-$UID/obsctl.sock`

Request examples:

```json
{"id":"req-000001","type":"command","command":{"name":"set_scene","target":"main"}}
{"id":"req-000002","type":"subscribe","topics":["state","events","logs"]}
```

Response examples:

```json
{"id":"req-000001","type":"response","ok":true,"result":{"message":"scene set: Main"}}
{"id":"req-000001","type":"response","ok":false,"error":{"code":"OBS_UNAVAILABLE","message":"OBS is unavailable"}}
```

Pushed event example:

```json
{"type":"event","topic":"state","data":{"connected":true,"current_scene":"Main"}}
```

Supported topics:

- `state`
- `events`
- `logs`

Supported command names:

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

## Server Design

The server owns:

- OBS WebSocket connection.
- OBS authentication.
- Reconnect loop.
- Authoritative OBS snapshot cache.
- Config loading and validation.
- Alias/shortcut resolution.
- Command execution.
- Dump-config merge/write logic.
- Unix socket IPC listener.
- Client session registry.
- State/event/log broadcasting.

Startup flow:

1. Resolve config path.
2. Load and validate config.
3. Resolve socket path.
4. Create runtime directory.
5. Check existing socket.
6. Remove stale socket only when no process responds.
7. Start IPC listener.
8. Start OBS supervisor task.
9. Attempt OBS connection.
10. Authenticate.
11. Fetch initial snapshot.
12. Broadcast state to subscribed clients.
13. Run until signal, IPC shutdown, or fatal setup error.

Shutdown flow:

- Stop accepting IPC clients.
- Close client sessions.
- Stop OBS supervisor.
- Close WebSocket.
- Remove socket file.
- Flush logs.

The server must stay alive while OBS is unavailable. Scene/audio commands return `OBS_UNAVAILABLE`, but `status`, `server-status`, `reload-config`, `validate-config`, and reconnect commands remain useful.

## OBS WebSocket Client

Target protocol: obs-websocket 5.x.

Connection sequence:

1. Connect WebSocket.
2. Receive Hello opcode `0`.
3. Send Identify opcode `1`.
4. Receive Identified opcode `2`.
5. Send Request opcode `6`.
6. Match RequestResponse opcode `7` by request ID.
7. Consume Event opcode `5`.

Authentication:

1. `secret = base64(sha256(password + salt))`
2. `authentication = base64(sha256(secret + challenge))`

Never log:

- Passwords.
- Authentication strings.
- Config fields containing secrets.

Required OBS requests:

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

Required event categories:

- General.
- Scenes.
- Inputs.

Do not subscribe to high-volume input meter events by default.

## State Model

Use typed structs serialized through Serde for IPC and tests.

```rust
struct ObsSnapshot {
    connected: bool,
    obs_studio_version: Option<String>,
    obs_websocket_version: Option<String>,
    current_scene: Option<String>,
    scenes: Vec<SceneState>,
    audio_inputs: Vec<AudioState>,
    last_error: Option<String>,
    updated_at: OffsetDateTime,
}

struct SceneState {
    name: String,
    alias: Option<String>,
    shortcut: Option<String>,
    group: Option<String>,
    active: bool,
}

struct AudioState {
    name: String,
    alias: Option<String>,
    shortcut: Option<String>,
    kind: Option<String>,
    muted: Option<bool>,
    volume_mul: Option<f64>,
    volume_db: Option<f64>,
    volume_percent: Option<u8>,
}

struct ServerStatus {
    pid: u32,
    uptime_seconds: u64,
    socket_path: PathBuf,
    client_count: usize,
    obs_connected: bool,
    reconnecting: bool,
    last_error: Option<String>,
}
```

`StateStore` should provide:

- Read current snapshot.
- Replace full snapshot.
- Apply OBS events.
- Mark disconnected with error.
- Broadcast only after state changes.

## TUI Design With Ratatui

The TUI is an IPC client. It must not contain OBS WebSocket logic.

Use:

- `ratatui::Terminal<CrosstermBackend<Stdout>>`
- Crossterm raw mode and alternate screen.
- A Tokio task for IPC subscription events.
- A terminal event loop for key input and periodic ticks.

Dashboard layout:

- Top header band: app name, daemon status, OBS status, versions.
- Left or upper panel: scenes with active highlight, alias, shortcut, group.
- Secondary scene map: grouped scene names and active marker.
- Audio panel: input name, alias, shortcut, mute state, volume percent.
- Log panel: recent daemon warnings/errors.
- Bottom command palette: editable command line and last result.

Expected key behavior:

- `/` or `:` opens the command palette.
- `Backspace` edits current palette input.
- `Enter` submits palette command.
- `Esc` or `Ctrl-C` cancels palette editing.
- `q` quits from dashboard mode.
- `r` sends `/reload-config`.
- `D` sends `/dump-config`.

Server unavailable screen:

- Show socket path.
- Show commands to start daemon and install service.
- Offer retry, start embedded server if supported by config, and quit.

Implementation note: Ratatui widgets should render from a `tui::Model` only. Commands should go through `tui::Session`, which delegates to IPC.

## CLI Design

Use `clap` for global options and subcommands.

Global options:

- `--config PATH`
- `--log-level debug|info|warn|error`
- `--force`

CLI commands that directly act locally:

- `init`
- `validate-config`
- `server`
- `server --headless`
- `service ...`
- `tui`

CLI commands that proxy to the server:

- `status`
- `obs-status`
- `server-status`
- `reconnect`
- `shutdown-server`
- `scene`
- `mute`
- `unmute`
- `toggle-mute`
- `vol|volume`
- `dump-config`
- `reload-config`

Exit codes:

- `0`: success.
- `1`: generic failure.
- `2`: config error.
- `3`: server/connection/auth error.
- `4`: OBS request error.
- `5`: command parse error.
- `6`: IPC error.

Server unavailable message:

```text
obsctl server is not running.
Start it with:
  obsctl server --headless
Or install service:
  obsctl service install
  systemctl --user enable --now obsctl.service
```

## Dump Config

`dump-config` must run inside the server because the server owns the OBS connection.

Behavior:

1. Use active OBS client or return `OBS_UNAVAILABLE`.
2. Fetch scene names.
3. Fetch audio input names.
4. Read existing config.
5. Preserve aliases, shortcuts, groups, stale flags, server settings, reconnect settings, UI settings, and keymap.
6. Mark configured entries not present in OBS as `stale: true`.
7. Add newly discovered OBS resources.
8. Validate duplicate aliases/shortcuts.
9. Validate alias/shortcut collisions with OBS names.
10. Write a timestamped backup.
11. Write atomically.
12. Reload server config and refresh/broadcast state.

## Systemd User Service

Commands:

- `obsctl service install`
- `obsctl service uninstall`
- `obsctl service status`
- `obsctl service start`
- `obsctl service stop`
- `obsctl service restart`

Unit file path:

```text
~/.config/systemd/user/obsctl.service
```

Template:

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

Rules:

- Do not use sudo.
- Use `systemctl --user`.
- Resolve the actual current executable path.
- Run `systemctl --user daemon-reload` after install and uninstall.

## Error Model

Define an `ObsctlError` enum with variants equivalent to:

- `ConfigNotFound`
- `ConfigInvalid`
- `ServerUnavailable`
- `IpcConnectionFailed`
- `IpcProtocolError`
- `ConnectionFailed`
- `AuthenticationFailed`
- `ObsUnavailable`
- `RequestTimeout`
- `ObsRequestFailed`
- `SceneNotFound`
- `AudioInputNotFound`
- `AliasAmbiguous`
- `CommandParseError`
- `DumpConfigFailed`
- `ServiceInstallFailed`

Every user-facing error should be concise, actionable, and secret-free.

## Logging

Default log path:

```text
~/.local/state/obsctl/obsctl.log
```

Logging rules:

- Support `--log-level debug|info|warn|error` for server mode.
- Persist server logs.
- Broadcast compact log events to subscribed TUI clients.
- Redact password/authentication fields.
- Debug logs may include request type, request ID, state transition, and socket path.

## Implementation Phases

### Phase 1: Project Foundation

- Create Cargo project.
- Add dependencies.
- Add module skeleton.
- Add formatting, clippy, and test commands.
- Add typed error and result model.

Acceptance:

- `cargo fmt --check`, `cargo clippy`, and `cargo test` run on an empty skeleton.

### Phase 2: Config and Domain

- Implement config structs and YAML load/write.
- Implement config path resolution.
- Implement schema validation and warnings.
- Implement command parser.
- Implement alias/shortcut resolution.
- Implement volume conversion.

Acceptance:

- Config tests cover defaults, legacy reconnect migration, unknown top-level rejection, password env validation, plaintext warning, duplicates, and canonical writes.
- Parser tests cover quoted names, escapes, invalid counts, unknown commands, and volume range.

### Phase 3: IPC

- Implement Unix socket path resolution.
- Implement newline JSON codec.
- Implement typed request/response/event models.
- Implement Unix client request/response.
- Implement Unix server accept loop and client sessions.
- Implement topic subscription registry.

Acceptance:

- Codec tests cover round-trip, malformed JSON, partial frames, multiple frames, and unknown messages.
- Integration tests prove a client can command, subscribe, and receive pushed state.

### Phase 4: OBS Client

- Implement obs-websocket opcodes and request models.
- Implement authentication.
- Implement WebSocket connection and Identify handshake.
- Implement request ID generation and response correlation.
- Implement request timeout handling.
- Implement event stream handling.
- Implement typed wrappers for version, scenes, and audio.
- Build a fake OBS WebSocket server for integration tests.

Acceptance:

- Auth test matches known vectors.
- Fake server tests cover unauthenticated and authenticated handshakes, request success, request failure, timeout, event delivery, and disconnect.

### Phase 5: Server

- Implement `StateStore`.
- Implement `ObsSupervisor`.
- Implement command executor.
- Implement reconnect policy.
- Implement IPC server lifecycle.
- Implement server status.
- Implement shutdown signal handling.
- Implement safe socket cleanup.

Acceptance:

- Server runs with OBS unavailable and responds to status.
- Scene/audio commands fail with `OBS_UNAVAILABLE` when appropriate.
- Reconnect attempts follow backoff.
- Subscribed TUI clients receive initial state and later broadcasts.

### Phase 6: CLI

- Implement `clap` command model.
- Implement local commands.
- Implement proxy command mapping.
- Implement server unavailable behavior.
- Implement exit code mapping.

Acceptance:

- CLI integration tests cover all commands with fake IPC server.
- `init` and `validate-config` work without daemon.
- Proxy commands never connect directly to OBS.

### Phase 7: Ratatui TUI

- Implement terminal setup/teardown.
- Implement TUI model and session.
- Implement IPC subscription handling.
- Implement command palette and keymap.
- Implement widgets.
- Implement server unavailable screen.
- Implement resize/tick handling.

Acceptance:

- Widget tests or snapshot tests cover connected, disconnected, long names, small terminal sizes, empty lists, and error states.
- Session tests prove commands go through IPC and update model from pushed events.

### Phase 8: Dump Config and Service Management

- Implement config dump merge logic.
- Implement timestamped backup and atomic write.
- Implement systemd user service installer.
- Implement service command runner abstraction for tests.

Acceptance:

- Dump tests cover preserve, add, stale, duplicate, collision, backup, and atomic write failure.
- Service tests cover unit content, executable path, daemon-reload, start/stop/restart/status/uninstall.

### Phase 9: Hardening

- Add tracing redaction tests.
- Add race tests around disconnect/reconnect and pending OBS requests.
- Add shutdown cleanup tests.
- Add documentation parity with current README/docs.
- Add release build workflow.

Acceptance:

- Full test suite passes.
- Manual smoke test with OBS Studio validates scene/audio control and TUI updates.

## Manual Smoke Test

With OBS Studio running and obs-websocket enabled:

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

## Migration Notes From Crystal

Carry forward:

- Daemon-owned OBS connection.
- Unix socket newline JSON IPC.
- Config schema and command grammar.
- Alias resolution behavior.
- Safe dump-config semantics.
- Server log redaction.
- Systemd user service behavior.

Improve:

- Replace direct ANSI rendering with Ratatui.
- Avoid retaining any duplicate direct-to-OBS command path.
- Use Serde for typed protocol boundaries.
- Use Tokio cancellation/shutdown primitives instead of loose task lifetimes.
- Make integration tests independent from real OBS through a fake obs-websocket server.
