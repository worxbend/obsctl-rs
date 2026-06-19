# obsctl-rs

A local OBS Studio controller for obs-websocket 5.x, written in Rust with Ratatui.

## Architecture

```
OBS Studio <── obs-websocket 5.x ──> obsctl server <── Unix socket IPC ──> obsctl tui
                                                    <── Unix socket IPC ──> obsctl CLI
```

**Hard rule:** only `obsctl server` connects directly to OBS. The CLI and TUI are thin IPC clients.

## Quick Start

```sh
cargo build --release

# Initialize config
./target/release/obsctl init

# Set your OBS WebSocket password
export OBS_WEBSOCKET_PASSWORD='your_password'

# Validate config
./target/release/obsctl validate-config

# Start the daemon
./target/release/obsctl server --headless

# Use CLI commands
./target/release/obsctl server-status
./target/release/obsctl obs-status
./target/release/obsctl dump-config
./target/release/obsctl scene 'Main'
./target/release/obsctl mute 'Mic'
./target/release/obsctl vol 'Mic' 70

# Launch the TUI dashboard
./target/release/obsctl tui
```

## Service Install (systemd --user)

```sh
obsctl service install
systemctl --user enable --now obsctl.service
obsctl service status
```

## Config

Default path: `~/.config/obsctl/config.yml`

Override: `OBSCTL_CONFIG=/path/to/config.yml` or `--config PATH`

```yaml
version: 1
server:
  socket_path:           # optional; defaults to $XDG_RUNTIME_DIR/obsctl/obsctl.sock
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

**Security:** never set `connection.password` in plain text. Use `password_env` to point to an environment variable name.

## CLI Commands

Global options:

- `--config PATH` or `OBSCTL_CONFIG=/path/to/config.yml` selects the config file.
- `--json` makes proxy command output a stable machine-readable JSON envelope.

### Local (no daemon required)

| Command | Description |
|---------|-------------|
| `obsctl init [--force]` | Create default config |
| `obsctl validate-config` | Validate config file |
| `obsctl server` | Start daemon in foreground |
| `obsctl server --headless` | Start daemon for service use |
| `obsctl tui` | Launch TUI dashboard |
| `obsctl service install\|uninstall\|start\|stop\|restart\|status` | Manage systemd user service |

### Proxy (require running daemon)

| Command | Description |
|---------|-------------|
| `obsctl status` | Show combined status |
| `obsctl server-status` | Show daemon status |
| `obsctl obs-status` | Show OBS connection status |
| `obsctl scene <target>` | Change current program scene |
| `obsctl mute <target>` | Mute audio input |
| `obsctl unmute <target>` | Unmute audio input |
| `obsctl toggle-mute <target>` | Toggle audio input mute |
| `obsctl vol <target> <0-100>` | Set input volume by percent |
| `obsctl volume <target> <0-100>` | Alias for `vol` |
| `obsctl dump-config` | Fetch OBS state and merge into config |
| `obsctl reload-config` | Reload config and rebroadcast state |
| `obsctl reconnect` | Ask daemon to reconnect to OBS |
| `obsctl shutdown-server` | Shut down daemon (requires `allow_remote_shutdown: true`) |

## TUI Key Bindings

| Key | Action |
|-----|--------|
| `/` or `:` | Open command palette |
| `Enter` | Submit palette command |
| `Esc` | Cancel palette editing |
| `q` | Quit |
| `r` | Reload config |
| `D` | Dump config |
| `Ctrl-C` | Quit or cancel palette |

### TUI Command Palette

Type `/` to open the palette, then use any of:

```
/help
/scene <target>
/set-scene <target>
/mute <target>
/unmute <target>
/toggle-mute <target>
/vol <target> <0-100>
/status
/server-status
/obs-status
/validate-config
/reconnect
/dump-config
/reload-config
/quit
```

## Alias Resolution

Target names resolve in this order:
1. Exact shortcut
2. Exact alias
3. Exact OBS name
4. Case-insensitive alias
5. Case-insensitive OBS name

Ambiguous matches fail without sending any OBS request.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Generic failure |
| 2 | Config error |
| 3 | Server/connection/auth error |
| 4 | OBS request error |
| 5 | Command parse error |
| 6 | IPC error |

There are two intentional exit-code mappings:

- Local process failures use the local error classification. These are failures before a daemon IPC response exists, such as `init`, `validate-config`, `server`, service management, startup, config loading, or socket connection setup.
- Proxy commands that receive a daemon response use the public IPC error-code table below. That table is the stable daemon-reachable contract for CLI and TUI clients.

These mappings are separate because the same underlying condition can have different process context. For example, a local startup/authentication failure exits as a server/connection failure, while a reachable daemon that cannot use OBS reports `OBS_UNAVAILABLE` to proxy clients.

## Observable CLI Contract

`obsctl` is daemon-first in normal use. Proxy commands connect to the local Unix socket, send one IPC command to the already-running daemon, wait for the correlated response, print the result, and exit. They do not connect directly to OBS and they do not auto-start the daemon.

Proxy commands include `status`, `server-status`, `obs-status`, `scene`, `mute`, `unmute`, `toggle-mute`, `vol`, `volume`, `dump-config`, `reload-config`, `reconnect`, and `shutdown-server`.

Without `--json`, command output is concise and human-readable. Successful command results are printed to stdout. Diagnostics and errors are printed to stderr.

With `--json`, stdout is the machine-readable contract and stderr is not used for human diagnostics. All proxy command outcomes use the same envelope with stable `ok`, `result`, `error`, and `exit_code` fields:

```json
{
  "ok": true,
  "result": {
    "message": "scene set: Main"
  },
  "error": null,
  "exit_code": 0
}
```

On failure, `ok` is `false`, `result` is `null`, and `error` contains a public IPC error code plus a secret-safe message. The `exit_code` field is the same code returned by the process:

```json
{
  "ok": false,
  "result": null,
  "error": {
    "code": "OBS_UNAVAILABLE",
    "message": "OBS is unavailable"
  },
  "exit_code": 4
}
```

If a future or third-party daemon returns an unknown `error.code`, the CLI preserves that string in the JSON envelope and exits `1`.

Config errors returned by the daemon keep the same envelope and exit with code `2`:

```json
{
  "ok": false,
  "result": null,
  "error": {
    "code": "CONFIG_INVALID",
    "message": "config invalid: invalid field"
  },
  "exit_code": 2
}
```

If no IPC response exists because the daemon is not reachable, `--json` still prints only the failure envelope to stdout:

```json
{
  "ok": false,
  "result": null,
  "error": {
    "code": "SERVER_UNAVAILABLE",
    "message": "obsctl server is not running.\nStart it with:\n  obsctl server --headless\nOr install the service:\n  obsctl service install\n  systemctl --user enable --now obsctl.service"
  },
  "exit_code": 3
}
```

In non-JSON mode, the same local server-unavailable case prints a human diagnostic to stderr:

```text
server unavailable at <socket-path>: obsctl server is not running.
Start it with:
  obsctl server --headless
Or install the service:
  obsctl service install
  systemctl --user enable --now obsctl.service
```

## IPC Error Codes

IPC error responses use this shape:

```json
{
  "id": "req-000001",
  "type": "response",
  "ok": false,
  "error": {
    "code": "REQUEST_TIMEOUT",
    "message": "request timed out"
  }
}
```

Public error codes are stable and map to CLI exit codes as follows:

| Code | Meaning | CLI exit |
|------|---------|----------|
| `CONFIG_INVALID` | Config file is missing, invalid, or failed validation | 2 |
| `SERVER_UNAVAILABLE` | Local daemon/socket connection failed before a valid command response | 3 |
| `OBS_UNAVAILABLE` | Daemon is reachable, but OBS is not currently connected or usable | 4 |
| `REQUEST_TIMEOUT` | An OBS request exceeded `connection.request_timeout_ms` | 4 |
| `OBS_REQUEST_FAILED` | OBS returned a request failure or the daemon could not process the OBS response | 4 |
| `SCENE_NOT_FOUND` | Scene target could not be resolved | 4 |
| `AUDIO_INPUT_NOT_FOUND` | Audio input target could not be resolved | 4 |
| `ALIAS_AMBIGUOUS` | Target matched more than one configured alias/name | 1 |
| `COMMAND_PARSE_ERROR` | Command name or arguments are invalid | 5 |
| `IPC_PROTOCOL_ERROR` | IPC frame or response shape is invalid for the protocol | 6 |
| `SHUTDOWN_DISABLED` | Remote daemon shutdown is disabled in config | 1 |
| `SERVER_ERROR` | Generic daemon-side failure | 1 |

`REQUEST_TIMEOUT` and `OBS_UNAVAILABLE` are intentionally distinct. `OBS_UNAVAILABLE` means the daemon cannot currently make OBS requests because OBS is disconnected, authentication failed, or the OBS connection is otherwise unavailable. `REQUEST_TIMEOUT` means a request was attempted against OBS, but no matching response arrived before `connection.request_timeout_ms`.

Bad subscription topics are protocol failures. A subscribe request containing any topic outside `state`, `events`, or `logs` returns `IPC_PROTOCOL_ERROR` with a message naming the unknown topic; `INVALID_TOPIC` is not emitted by the frozen wire contract.

Error messages are intended to be actionable and secret-safe. They must not include OBS passwords, authentication strings, bearer tokens, or resolved secret environment-variable values.

## IPC Protocol

Transport is newline-delimited JSON over a Unix domain socket.

Command request:

```json
{"id":"req-000001","type":"command","command":{"name":"set_scene","target":"Main"}}
```

Subscribe request:

```json
{"id":"req-000002","type":"subscribe","topics":["state","events","logs"]}
```

Success response:

```json
{"id":"req-000001","type":"response","ok":true,"result":{"message":"scene set: Main"}}
```

Error response:

```json
{"id":"req-000001","type":"response","ok":false,"error":{"code":"OBS_UNAVAILABLE","message":"OBS is unavailable"}}
```

State event:

```json
{"type":"event","topic":"state","data":{"connected":true}}
```

OBS event:

```json
{"type":"event","topic":"events","data":{"type":"CurrentProgramSceneChanged","scene_name":"BRB"}}
```

Typed log event:

```json
{"type":"event","topic":"logs","data":{"level":"info","message":"daemon listening","target":"obsctl_rs::server","timestamp":"1970-01-01T00:00:00Z"}}
```

OBS events use normalized typed payloads derived from the internal OBS event model, not raw obs-websocket event envelopes. Audio events use the same convention, for example `{"type":"InputMuteStateChanged","input_name":"Mic","muted":true}` or `{"type":"InputVolumeChanged","input_name":"Desktop Audio","volume_mul":0.75,"volume_db":-2.5}`.

For log events, `level` is one of `trace`, `debug`, `info`, `warn`, or `error`; `target` may be omitted; and `timestamp` is RFC3339 UTC. Supported event topics are `state`, `events`, and `logs`.

Error and log messages are redacted by a best-effort boundary sanitizer. Prefer structured non-secret fields over formatted messages that include secret-bearing values.

Compatibility note: `INVALID_TOPIC` is not a public wire code in this release. Clients that previously treated invalid subscription topics specially should handle `IPC_PROTOCOL_ERROR` for that case. No other public wire code is intentionally renamed or removed.

## Logging

Default log path: `~/.local/state/obsctl/obsctl.log`

Control level with `--log-level debug|info|warn|error` in server mode.

Passwords and authentication strings are never logged.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

Tests use a fake OBS WebSocket server and fake IPC servers — no real OBS required.
