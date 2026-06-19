# obsctl-rs — Next Iteration Plan

**Generated:** 2026-06-19  
**Context:** 10-phase build complete. 164 tests passing. Core architecture stable.

---

## Current State Summary

The project has a fully implemented core:

- **IPC daemon** with Unix socket, JSON codec, broadcast topics, reconnect loop
- **OBS WebSocket client** (obs-websocket 5.x) with auth, request/event dispatch
- **CLI** via `clap` — local + proxy commands, exit codes, server-unavailable UX
- **Ratatui TUI** — scenes, audio, logs, command palette, header, connection panel
- **Config** — YAML schema, dump/merge, atomic write, backup, legacy migration
- **Service** — systemd --user install/uninstall/start/stop/restart/status
- **Test suite** — 164 tests: unit, IPC, OBS client, server, TUI widget, CLI integration

Known gaps and debt:

- `reload_config` in `server/command_executor.rs:342` logs a warning and does nothing
- `#![allow(dead_code, unused_imports, ...)]` suppresses real issues in `lib.rs:1`
- No real OBS recording/streaming control (start/stop/pause)
- No scene transitions or transition types
- No source/filter manipulation
- No `obsctl` shell completions
- TUI has no mouse support
- No configurable themes beyond `"default"` sentinel
- No metrics/stats endpoint
- No HTTP API (future remote access)
- Audio volume bar is text-only; no visual bar
- No hotkey passthrough from TUI to OBS
- No batch command support in CLI
- No `--json` output flag for scripting
- No OBS screenshot/capture command
- No scene-collection switching

---

## Improvement Areas

### I. Code Quality

#### I-1. Remove `#![allow(...)]` Suppressions

`src/lib.rs:1` suppresses `dead_code`, `unused_imports`, `unused_variables`, and `unused_mut` globally.  
These mask real issues. Remove the allow and fix every compiler warning individually.

- **Effort:** Medium  
- **Value:** Reveals real dead paths, improves maintainability

#### I-2. Wire `reload_config` Command

`server/command_executor.rs:341-344` logs a warning and returns a fake OK.  
The config path is already stored in `CommandExecutor.config_path`. Use `config::loader::load()` and update `self.config`, then broadcast the refreshed state snapshot to subscribers.

- **Effort:** Small  
- **Value:** Closes a gap between README promise and actual behavior

#### I-3. Request Timeout in `ObsClient`

`obs/client.rs` has no timeout on pending requests. If OBS never replies, the oneshot sender sits in the pending map forever. Add a `tokio::time::timeout` wrapper in `ObsClient::request()` using `config.connection.request_timeout_ms`.

- **Effort:** Small  
- **Value:** Prevents silent hangs under partial OBS failure

#### I-4. Structured Log Events Over IPC

Currently log events broadcast raw strings. Define a typed `LogEvent` struct in `ipc/protocol.rs` with `level`, `timestamp`, `message` fields so TUI can color-code by severity without string parsing.

```rust
pub struct LogEvent {
    pub level: LogLevel,   // Debug | Info | Warn | Error
    pub ts: OffsetDateTime,
    pub message: String,
}
```

- **Effort:** Medium  
- **Value:** Enables severity-colored log panel in TUI

#### I-5. Error Context in IPC Responses

`domain/errors.rs` error messages are terse. Add optional `hint` field to `ErrorPayload` so the TUI/CLI can show actionable suggestions inline, e.g., "scene not found — run `dump-config` to sync".

- **Effort:** Small  
- **Value:** Better user experience in TUI status bar

---

### II. OBS Feature Coverage

#### II-1. Recording Control

Add OBS requests and CLI/TUI commands to start, stop, and pause recording.

**New CLI commands:**

```
obsctl record start
obsctl record stop
obsctl record pause
obsctl record resume
obsctl record status
```

**New OBS requests** (`obs/requests.rs`):

- `StartRecord`
- `StopRecord`
- `PauseRecord`
- `ResumeRecord`
- `GetRecordStatus`

**New IPC command names:** `start_record`, `stop_record`, `pause_record`, `resume_record`, `get_record_status`

**TUI:** Add `[REC]` indicator with red pulsing style in header widget when recording is active.

- **Effort:** Medium  
- **Value:** High — recording control is the #1 missing feature for streamers

#### II-2. Streaming Control

Mirror recording control for streaming.

**New CLI commands:**

```
obsctl stream start
obsctl stream stop
obsctl stream status
```

**New OBS requests:** `StartStream`, `StopStream`, `GetStreamStatus`

**TUI:** Add `[LIVE]` badge with elapsed time in header widget when streaming.

- **Effort:** Medium  
- **Value:** High — streaming control is the core OBS use case

#### II-3. Scene Transitions

Allow setting the current transition and its duration before switching scenes.

**New CLI commands:**

```
obsctl transition <name>
obsctl transition-duration <ms>
obsctl list-transitions
```

**New OBS requests:** `GetTransitionKindList`, `GetCurrentSceneTransition`, `SetCurrentSceneTransition`, `SetCurrentSceneTransitionDuration`

- **Effort:** Medium  
- **Value:** Medium — transition control is important for broadcast polish

#### II-4. Source Visibility Toggle

Toggle visibility of sources within a scene.

**New CLI commands:**

```
obsctl show <scene> <source>
obsctl hide <scene> <source>
obsctl toggle-source <scene> <source>
```

**New OBS requests:** `GetSceneItemList`, `GetSceneItemEnabled`, `SetSceneItemEnabled`

- **Effort:** Medium  
- **Value:** High — allows dynamic layouts from scripts

#### II-5. Screenshot / Capture

Take a screenshot of a source or the program output.

**New CLI command:**

```
obsctl screenshot [--source <name>] [--output <path>]
```

**New OBS request:** `GetSourceScreenshot`, `SaveSourceScreenshot`

- **Effort:** Small  
- **Value:** Useful for CI/QA pipelines and livestream previews

#### II-6. Scene Collection Switching

**New CLI commands:**

```
obsctl collection list
obsctl collection switch <name>
```

**New OBS requests:** `GetSceneCollectionList`, `SetCurrentSceneCollection`

- **Effort:** Small  
- **Value:** Multi-show/setup support

#### II-7. Profile Switching

**New CLI commands:**

```
obsctl profile list
obsctl profile switch <name>
```

**New OBS requests:** `GetProfileList`, `SetCurrentProfile`

- **Effort:** Small  
- **Value:** Multi-setup support for users with different recording configs

---

### III. TUI Enhancements

#### III-1. Volume Bar Widget

Replace the text-only `42%` volume display in the audio panel with an inline block bar:

```
🔊 Mic/Line (mic)  [████████░░]  84%
```

Use Ratatui `Gauge` or a custom `Line` of `█`/`░` characters. Draw proportionally to available width.

- **Effort:** Small  
- **Value:** Immediately visual, professional feel

#### III-2. Recording/Streaming Status in Header

Extend the header widget (`tui/widgets/header.rs`) to show:

- `[LIVE 01:23:45]` in red when streaming, with elapsed time
- `[REC]` in red when recording
- `[PAUSED]` in yellow when paused

Use a server-pushed `RecordingStatus` and `StreamingStatus` event pushed as `state` topic updates.

- **Effort:** Medium  
- **Value:** Core streamer UX

#### III-3. Mouse Support

Add mouse event handling in `tui/app.rs`. Targets:

- Click on scene in scenes panel → `set_scene`
- Click on audio input → `toggle_mute`
- Scroll on audio input → `set_volume ±5`

Enable `crossterm::event::EnableMouseCapture` and `DisableMouseCapture` in terminal setup/teardown.

- **Effort:** Medium  
- **Value:** Intuitive for users not fluent in keyboard shortcuts

#### III-4. Command History in Palette

The command palette has no history. Add `Vec<String>` to `TuiModel` and cycle with arrow up/down when palette is open. Persist history to `~/.local/state/obsctl/history` (max 100 lines).

- **Effort:** Small  
- **Value:** Quality-of-life for power users

#### III-5. Filterable Scene List

When many scenes exist, keyboard filtering speeds navigation. When palette is closed, typing letters filters the scene list in real time. Press `Enter` on highlighted scene to switch.

- **Effort:** Medium  
- **Value:** Usable with large OBS setups (20+ scenes)

#### III-6. Theme Support

The config `ui.theme` field is parsed but ignored. Implement at minimum:

- `"default"` — current color scheme
- `"dark"` — high contrast dark
- `"light"` — light terminal backgrounds
- `"catppuccin"` / `"gruvbox"` — popular terminal palettes

Define a `Theme` struct in `tui/` with color constants and thread it through all widget render calls.

- **Effort:** Medium  
- **Value:** Accessibility + personal preference

#### III-7. Resizable Pane Layout

Currently layout is fixed. Add toggle keybinds to expand/collapse individual panes:

- `s` — toggle scenes pane expansion
- `a` — toggle audio pane expansion
- `l` — toggle log pane expansion

Store layout state in `TuiModel`. Useful on narrow terminals.

- **Effort:** Medium  
- **Value:** Ergonomics on varied screen sizes

---

### IV. CLI & Scripting Enhancements

#### IV-1. `--json` Output Flag

Add `--json` as a global flag. When set, all proxy commands emit machine-readable JSON to stdout instead of human-readable text. Enables piping into `jq`.

```
obsctl --json obs-status | jq '.current_scene'
```

- **Effort:** Small  
- **Value:** High for scripting, CI, and automation

#### IV-2. Shell Completion Generation

Add `obsctl completions <shell>` command using `clap_complete`:

```
obsctl completions bash > ~/.local/share/bash-completion/completions/obsctl
obsctl completions zsh > ~/.zfunc/_obsctl
obsctl completions fish > ~/.config/fish/completions/obsctl.fish
```

- **Effort:** Small  
- **Value:** High polish, reduces onboarding friction

#### IV-3. Batch Command File Execution

```
obsctl run-script <file.obsctl>
```

Script file format — one command per line:

```
scene Main
mute Mic
vol Desktop 80
record start
```

Stops on first error by default; `--continue-on-error` flag available.

- **Effort:** Medium  
- **Value:** Macro/preset automation for streamers

#### IV-4. Watch Mode

```
obsctl watch
```

Streams a compact live view of OBS state to stdout — current scene, mute states, recording status — updating in place with ANSI cursor movement. Useful for status bars (waybar, polybar, etc.) or tmux status lines.

- **Effort:** Medium  
- **Value:** Headless monitoring without launching the full TUI

#### IV-5. `obsctl status --watch` One-liner

Variant of above — poll-based, no IPC subscription, usable in shell scripts:

```
obsctl status --watch --interval 2
```

Outputs one JSON line per interval. Exits on SIGINT.

- **Effort:** Small  
- **Value:** Compatibility with non-TUI automation setups

---

### V. Architecture & Infrastructure

#### V-1. HTTP API (Remote Control)

Add an optional HTTP API server alongside the Unix socket IPC. Useful for OBS control from scripts on other machines, browser-based dashboards, or Elgato Stream Deck plugins.

**Config addition:**

```yaml
http_api:
  enabled: false
  bind: "127.0.0.1:4456"
  token: ""   # Bearer token for auth
```

**Endpoints:**

```
GET  /status
GET  /snapshot
POST /scene    {"target": "Main"}
POST /mute     {"target": "Mic"}
POST /unmute   {"target": "Mic"}
POST /vol      {"target": "Mic", "percent": 70}
POST /record/start
POST /record/stop
POST /stream/start
POST /stream/stop
```

Implement using `axum` or `warp`. Keep it thin — delegate to `CommandExecutor`.

- **Effort:** Large  
- **Value:** High — enables integration with Stream Deck, OBS overlays, web dashboards

#### V-2. Plugin/Hook System (Script Triggers)

Let users define shell commands that run when OBS events fire:

```yaml
hooks:
  on_scene_change:
    - "notify-send 'Scene: {scene_name}'"
  on_recording_start:
    - "~/scripts/obs-started.sh"
  on_stream_live:
    - "discord-notify.sh 'Going live!'"
```

Spawn as background `tokio::process::Command`. No blocking. Capture stdout/stderr to log.

- **Effort:** Medium  
- **Value:** High — power-user automation without requiring a plugin API

#### V-3. Config Hot-Reload via `inotify`

Watch the config file with `notify` crate. On write event, re-validate and broadcast new state without requiring `obsctl reload-config`. Debounce 500ms.

- **Effort:** Small  
- **Value:** Developer experience — config changes take effect immediately

#### V-4. Metrics Endpoint

Expose a `GET /metrics` endpoint (Prometheus text format) if HTTP API is enabled:

```
obsctl_obs_connected{} 1
obsctl_scene_switches_total{} 42
obsctl_current_scene{name="Main"} 1
obsctl_audio_volume{input="Mic"} 0.84
obsctl_audio_muted{input="Mic"} 0
obsctl_recording_active{} 1
obsctl_streaming_active{} 0
obsctl_uptime_seconds{} 3601
```

- **Effort:** Medium  
- **Value:** Enables Grafana dashboards, alerting, and stream analytics

#### V-5. Multi-OBS Connection Support

Allow the daemon to manage connections to multiple OBS instances (e.g., main machine + laptop NDI source):

```yaml
connections:
  - id: "main"
    host: "127.0.0.1"
    port: 4455
    password_env: "OBS_MAIN_PASSWORD"
  - id: "laptop"
    host: "192.168.1.100"
    port: 4455
    password_env: "OBS_LAPTOP_PASSWORD"
```

CLI targets scoped: `obsctl --obs main scene Main`

- **Effort:** Large  
- **Value:** Pro/broadcast use cases with multi-PC setups

---

### VI. Developer Experience

#### VI-1. `obsctl doctor` Command

Diagnostic command that checks the full system and prints a status report:

```
obsctl doctor

  [OK]  Config found: ~/.config/obsctl/config.yml
  [OK]  Config valid
  [OK]  Daemon running (pid 12345, uptime 3h 42m)
  [OK]  OBS connected (v31.0.0, ws v5.5.4)
  [WARN] OBS_WEBSOCKET_PASSWORD is set but ignored (no-auth mode)
  [OK]  systemd service enabled and active
  [OK]  Socket: /run/user/1000/obsctl/obsctl.sock
```

- **Effort:** Small  
- **Value:** Eliminates most "why doesn't it work" support questions

#### VI-2. `obsctl config edit` Command

Open config in `$EDITOR`:

```
obsctl config edit
```

After editor closes, validate the new config. If invalid, offer to revert to the backup.

- **Effort:** Small  
- **Value:** Common workflow, avoids manual path lookup

#### VI-3. Integration Test Coverage for New Commands

For every new OBS command added (record, stream, transition, etc.), add:

1. A fake OBS server handler in `tests/support/fake_obs_server.rs`
2. At least 2 integration tests in `tests/obs_client_integration.rs`
3. At least 1 CLI integration test in `tests/cli_integration.rs`

This is a process requirement, not a single deliverable.

#### VI-4. Insta Snapshot Tests for TUI Widgets

Replace fragile manual string assertions in `tests/tui_widget_rendering.rs` with `insta` snapshot tests:

```rust
insta::assert_snapshot!(render_to_string(area, &model));
```

Snapshots live in `tests/snapshots/`. `cargo insta review` updates them interactively.

- **Effort:** Small  
- **Value:** Makes TUI widget changes reviewable and safe to refactor

---

## Prioritized Delivery Phases

### Phase 10 — Debt Clearance & Foundation (do first)

1. Remove `#![allow(...)]` in `lib.rs`, fix all warnings
2. Wire `reload_config` to actually reload from disk
3. Add request timeout to `ObsClient::request()`
4. Structured `LogEvent` type in IPC protocol
5. `--json` output flag for CLI

**Acceptance:** All 164 existing tests pass. `cargo clippy -D warnings` passes clean. `reload_config` verified via integration test.

---

### Phase 11 — OBS Core Feature Completeness

1. Recording control (start/stop/pause/resume/status)
2. Streaming control (start/stop/status)
3. Recording + streaming state in `ObsSnapshot` and `StateStore`
4. `[LIVE]` and `[REC]` in TUI header widget
5. Scene transitions (list/set/duration)
6. Source visibility toggle (show/hide/toggle-source)

**Acceptance:** Each command has fake OBS server tests and CLI integration tests. TUI header displays live/rec state.

---

### Phase 12 — TUI Polish

1. Volume bar widget (block characters, proportional to width)
2. Mouse support (click scene → switch, click audio → toggle mute, scroll → volume)
3. Command history in palette (arrow up/down, persist to disk)
4. Filterable scene list (type to filter)
5. Theme system (default/dark/light, catppuccin optional)

**Acceptance:** Insta snapshot tests for all updated widgets. Manual smoke test on 80×24 and 220×50 terminal.

---

### Phase 13 — Scripting & Automation

1. Shell completion generation (`obsctl completions <shell>`)
2. `obsctl run-script <file>` batch execution
3. `obsctl watch` streaming status output
4. Plugin/hook system (on_scene_change, on_recording_start, etc.)
5. Config hot-reload via `inotify`

**Acceptance:** `completions bash` generates valid completion script. `run-script` runs a 5-command file against fake server. Hook fires shell command on scene change in integration test.

---

### Phase 14 — HTTP API & Observability

1. HTTP API with `axum` behind `http_api.enabled` config flag
2. Bearer token auth on HTTP API
3. Prometheus metrics endpoint
4. `obsctl doctor` diagnostic command
5. `obsctl config edit` command

**Acceptance:** `curl /status` returns JSON. `curl /metrics` returns Prometheus text. `doctor` prints structured report. HTTP off by default — no behavior change for existing users.

---

### Phase 15 — Advanced Features

1. Screenshot/capture command
2. Scene collection switching
3. Profile switching
4. `obsctl status --watch` poll mode
5. Multi-OBS connection support (config schema + routing)

**Acceptance:** Screenshot saves PNG to disk. Collection/profile switch verified against fake server. Multi-OBS routing tested with two fake servers in same integration test.

---

## Restart Loop Entry

At the end of Phase 15 (or at any checkpoint where this plan has been executed):

1. Run `cargo test --all-targets --all-features` and collect the pass count.
2. Read `AGENT_LOG.md` for the final iteration summary.
3. **Generate a new `plan.md`** using the same methodology:
   - Audit the current source tree for new gaps and debt
   - Review the OBS WebSocket 5.x protocol changelog for newly supported requests
   - Review Ratatui release notes for new widget primitives
   - Propose the next 5 delivery phases, each with acceptance criteria
   - Append a restart loop entry at the end

This loop ensures `obsctl-rs` continuously improves in alignment with the upstream OBS ecosystem, user needs, and the evolving Rust crate landscape.

**Trigger command for next plan generation:**

```
claude "Read plan.md, AGENT_LOG.md, and all src/**/*.rs files, then generate a new plan.md that extends the work from the last completed phase. Include code quality improvements, new OBS feature coverage based on obs-websocket 5.x capabilities not yet implemented, TUI enhancements, and at least 3 creative new product ideas. End the plan with a restart loop entry."
```
