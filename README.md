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

## Daemon Unavailable

If the daemon is not running, proxy commands print:

```
obsctl server is not running.
Start it with:
  obsctl server --headless
Or install service:
  obsctl service install
  systemctl --user enable --now obsctl.service
```

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
