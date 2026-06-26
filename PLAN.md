# obsctl-rs Refactoring Plan

Concrete, actionable improvements grouped by theme. Each item names the file, the function/lines
affected, the problem, and the fix. Priority is H/M/L.

---

## 1. Duplication — extract repeated patterns

### H — `tui/app.rs`: duplicated session-forwarder spawn block
`run_loop` lines 54–68 and `TuiAction::RetryConnect` lines 203–217 are identical
`tokio::spawn` loops that forward `TuiEventSession` events into `ipc_tx`.
```rust
// extract to:
fn spawn_session_forwarder(
    mut session: TuiEventSession,
    tx: mpsc::Sender<Result<ServerMessage, String>>,
)
```
Both call sites become one line.

### H — `tui/app.rs`: triplicated response-message extractor
`send_simple` (line 421), `send_simple_with_target` (line 367), `send_set_volume` (line 394)
all contain the same match arm:
```rust
Ok(ServerMessage::Response { ok, result, error, .. }) => {
    if ok { result.get("message")... } else { error.map(...)... }
}
Ok(_) => "unexpected response"
Err(e) => format!("error: {e}")
```
Extract:
```rust
fn format_ipc_response(res: Result<ServerMessage, impl Display>) -> String
```
Callers become 1–2 lines each.

### H — `server/command_executor.rs`: repeated `AliasEntry` collection
`cmd_set_scene` (line 150), `cmd_set_mute` (line 180), `cmd_toggle_mute` (line 206),
`cmd_set_volume` (line 242) each build a `Vec<AliasEntry>` from the snapshot with identical
`.iter().map(|x| AliasEntry { name: x.name.clone(), alias: x.alias.clone(), shortcut: x.shortcut.clone() }).collect()`.
Extract two helpers on `ObsSnapshot` or `CommandExecutor`:
```rust
fn scene_alias_entries(snap: &ObsSnapshot) -> Vec<AliasEntry>
fn audio_alias_entries(snap: &ObsSnapshot) -> Vec<AliasEntry>
```

### M — `config/dump.rs`: near-identical duplicate validation fns
`validate_scene_duplicates` and `validate_audio_duplicates` (lines 138–180) share 95% of their
body. Merge into a single generic:
```rust
fn validate_no_duplicates(
    label: &str,
    items: impl Iterator<Item = (Option<&str>, Option<&str>)>, // (alias, shortcut)
) -> Result<()>
```

### M — `config/schema.rs`: same alias/shortcut uniqueness loop duplicated for scenes and audio
`validate` (lines ~89–131) iterates scenes and audio inputs checking for duplicate aliases and
shortcuts with near-identical code. Extract:
```rust
fn check_unique_aliases_shortcuts(
    kind: &str,
    items: &[impl HasAliasShortcut],
) -> Vec<ConfigWarning>
```

### M — `obs/requests.rs`: verbose `RequestData` constructors
Every function builds `RequestData { request_type: "X".to_string(), request_id: next_id(), request_data: None }`.
Add two package-private helpers:
```rust
fn req(type_: &str) -> RequestData
fn req_with(type_: &str, data: Value) -> RequestData
```
Reduces each public fn to 1 line.

---

## 2. Architecture / separation of concerns

### H — `tui/app.rs`: command dispatch spread across `run_loop`
The `match action { ... }` arm (lines 108–225) is 120 lines of inline command dispatch inside the
event loop. Move it to a dedicated `fn handle_action(action: TuiAction, model: &mut TuiModel, socket_path: &Path) -> impl Future<...>` so `run_loop` stays a pure event router.

### M — `server/command_executor.rs`: two consecutive `state.read().await` in `cmd_server_status`
Lines 122 and 124 each acquire a read lock independently, risking a race window. Read once:
```rust
let snap = self.state.read().await;
let last_error = snap.last_error.clone();
let obs_connected = snap.connected;
```

### M — `domain/command.rs`: `Disconnect` variant is dead
`Command::Disconnect` is parsed (parser.rs) and mapped to `"reconnect_obs"` in `app.rs`
`command_to_payload` — same target as `Command::Reconnect`. Either remove `Disconnect` or give it
distinct semantics. As-is it is misleading dead weight.

### M — `tui/model.rs`: `scenes()` allocates on every call
`scenes()` returns `Vec<&'_ SceneState>` built fresh each call. For the TUI's render path this runs
multiple times per frame. Two options:
1. Cache the filtered list in `TuiModel` and rebuild it only when `clamp_cursors()` is called
   (i.e., after a snapshot update).
2. Return an iterator instead of allocating a `Vec`.

### L — `tui/event_applier.rs`: `TOPIC_EVENTS` variants other than `InputVolumeMeters` silently dropped
The wildcard `_ => {}` on line ~40 ignores all non-meter event payloads. Add a `tracing::debug!`
for unhandled variants so future additions are easier to discover.

### L — `obs/client.rs`: magic number `65613` for event subscription bitmask
The value is explained in a comment, but it is easy to corrupt on future additions. Define named
constants and combine them:
```rust
const ES_GENERAL: u32 = 1;
const ES_SCENES: u32 = 4;
const ES_INPUTS: u32 = 8;
const ES_OUTPUTS: u32 = 64;
const ES_INPUT_VOLUME_METERS: u32 = 65536;
const EVENT_SUBSCRIPTIONS: u32 =
    ES_GENERAL | ES_SCENES | ES_INPUTS | ES_OUTPUTS | ES_INPUT_VOLUME_METERS;
```

---

## 3. Rust idioms

### M — `server/command_executor.rs`: `.map(|s| s.to_string())` on `as_str()` result
`required_string` (line 448):
```rust
args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
```
Replace with:
```rust
args.get(key).and_then(|v| v.as_str()).map(str::to_owned)
```

### M — `config/dump.rs` / `config/schema.rs`: redundant `.clone()` when inserting into `HashSet<String>`
Strings are already owned; inserting a clone and then dropping the original wastes an allocation.
Pattern: `seen.insert(alias.clone())` where `alias: String`. Use `seen.insert(alias)` and move,
or keep the variable for re-use and use `seen.insert(alias.clone())` only when the value is
genuinely reused.

### M — `domain/parser.rs`: `current.clone()` then `current.clear()`
The loop (lines ~130) does `tokens.push(current.clone()); current.clear()`. This clones and then
clears. Use `std::mem::take` instead:
```rust
tokens.push(std::mem::take(&mut current));
```
Avoids the allocation of a second `String`.

### M — `ipc/unix_server.rs` / `ipc/protocol.rs`: `TOPIC_*.to_string()` on `&'static str` constants
`TOPIC_STATE`, `TOPIC_EVENTS`, `TOPIC_LOGS` are `&'static str`. Sites that call `.to_string()` on
them in hot paths (subscribe message handling) allocate unnecessarily. Either:
- Change the topic fields in the relevant structs to `Cow<'static, str>` and use `Cow::Borrowed`,
- Or accept `&str` comparisons directly where the only use is equality checking.

### L — `cli/router.rs`: unnecessary `level.clone()` on returned `String`
Line ~62: `return level.clone()` where `level` is a local `String` that is not used after the
return. Remove `.clone()` and move.

### L — `tui/widgets/scenes.rs`, `audio.rs`: `s.name.clone()` for ratatui `Span::raw`
`Span::raw` accepts `Into<Cow<'_, str>>` — pass a reference `s.name.as_str()` instead of cloning.
Same applies to alias/shortcut spans in both widgets.

---

## 4. Code organisation

### M — `tui/app.rs`: `render_unavailable` is inline in `app.rs`
This is a standalone UI widget; move it to `tui/widgets/connection.rs` (which already handles the
connected-but-OBS-unavailable case) or a new `tui/widgets/unavailable.rs`, keeping `app.rs` as a
pure loop.

### M — `server/command_executor.rs`: `CommandExecutor::new` takes 9 arguments
Use a builder or a dedicated `CommandExecutorConfig` struct:
```rust
pub struct CommandExecutorConfig {
    pub state: StateStore,
    pub obs: Arc<Mutex<Option<ObsClient>>>,
    pub config: Arc<Mutex<Config>>,
    pub config_path: Option<PathBuf>,
    pub socket_path: PathBuf,
    pub registry: ClientRegistry,
    pub reconnect_tx: mpsc::Sender<()>,
    pub shutdown_tx: watch::Sender<bool>,
    pub hub: Arc<BroadcastHub>,
}
```
Removes the `#[allow(clippy::too_many_arguments)]` suppression.

### L — `tui/model.rs`: `MAX_TUI_LOG_ENTRIES` is a module-level constant not tied to `TuiModel`
Move it inside `impl TuiModel` as an associated constant:
```rust
impl TuiModel {
    pub const MAX_LOG_ENTRIES: usize = 200;
}
```

### L — `config/model.rs`: `SceneConfig` and `AudioInputConfig` share `alias`/`shortcut`/`stale` fields
Consider a shared `ResourceMetadata { alias, shortcut, group, stale, hidden }` type or a trait
`HasAlias` to unify validation and alias-resolution code. Not urgent but removes friction when
adding future per-resource fields.

---

## 5. Error handling

### M — `server/command_executor.rs`: `cmd_dump_config` silently succeeds when `config_path` is `None`
Line 353: the `else` branch logs a warning and then the function returns success with scene/input
counts. A caller invoking `dump_config` without a config path will get an `ok` response but nothing
was written. Return `Err(ObsctlError::ConfigInvalid("no config path"))` instead.

### M — `server/state_store.rs`: `serde_json::to_value` failure silently ignored in `replace`
`build_snapshot` is infallible but callers that derive a snapshot from OBS data use
`serde_json::to_value` internally and discard errors with `unwrap_or_default`. Propagate with `?`
and surface to the supervisor log.

### L — `tui/app.rs`: daemon connection error stored in `last_result` not `logs`
When the initial connection fails (line 72), the error goes to `model.last_result` which is a
one-line status bar. It should also be pushed to `model.logs` so it persists in the log panel after
the user interacts with the command palette.

---

## 6. Test quality

### M — `ipc/protocol.rs`: `obs_event_wire_json_covers_public_payload_variants` should enumerate all variants
The test explicitly lists variants but doesn't assert that the list is exhaustive. If a new
`ObsEventPayload` variant is added without a test case, the gap goes unnoticed. Add a compile-time
check (e.g. a `const _: () = assert!(VARIANT_COUNT == 6)` pattern or a `#[deny(unused_variables)]`
match to force exhaustion).

### M — `config/schema.rs` / `domain/errors.rs`: hardcoded variant counts as `usize` constants
`OBSCTL_ERROR_VARIANT_COUNT` and similar are manually maintained. These are correct today but will
silently rot. Replace with a `strum::EnumCount` derive or a compile-time exhaustive match that will
fail to compile if a variant is added.

### L — `server/command_executor.rs`: `cmd_toggle_stream` / `cmd_toggle_record` not unit tested
The new toggle commands have no unit tests. Add tests in the same pattern as
`apply_event_scene_change` in `state_store.rs`: mock an `ObsClient`, verify the correct
`RequestData` is sent and the returned message matches the active state.

### L — `tui/model.rs`: `scenes()` filter not covered by a test
The `hidden: true` filter in `scenes()` has no dedicated test. Add a unit test that verifies hidden
scenes are excluded from the returned list and that cursor math stays valid when all visible scenes
are hidden.

---

## 7. Feature — Command Palette autocompletion

### Overview

Two-phase completion triggered by `Tab` / `Shift+Tab` while the palette is active:

1. **Command completion** — user types `/sc` → candidates are `/scene`; `/m` → `/mute`, `/mute`; empty or `/` → all commands listed.
2. **Argument completion** — user types `/scene <prefix>` → scene names and aliases matching prefix; `/mute <prefix>` / `/vol <prefix>` → audio input names and aliases matching prefix.

Completing a candidate writes it back into the input buffer so the user can keep typing.
Pressing `Enter` submits as usual; `Esc` closes the palette and discards the suggestion row.

---

### New file: `tui/completion.rs`

Single public function with no side effects — pure computation from input + model snapshot:

```rust
pub fn compute(input: &str, model: &TuiModel) -> Vec<String>
```

**Phase 1 — command prefix** (no space yet after the command word):
```
ALL_COMMANDS = ["/scene", "/mute", "/unmute", "/toggle-mute", "/vol",
                "/stream", "/rec", "/status", "/obs-status",
                "/reload-config", "/dump-config", "/reconnect", "/quit"]
```
Filter to those that start with `input` (case-insensitive). Return the filtered list.

**Phase 2 — argument prefix** (input contains a space after a recognised command):
- Split on first space → `(cmd, arg_prefix)`.
- `/scene <prefix>` → iterate `model.scenes()`, collect names and aliases, filter by `starts_with(arg_prefix)` (case-insensitive), de-duplicate, return as `"/scene <candidate>"` strings.
- `/mute`, `/unmute`, `/toggle-mute`, `/vol <prefix>` → same pattern over `model.audio_inputs()`.
- Any other command → empty list (no argument completion defined).

**Tie-breaking / ordering:** exact-prefix matches first, then alphabetical.

---

### State changes: `tui/model.rs` — `CommandPaletteState`

```rust
#[derive(Debug, Clone, Default)]
pub struct CommandPaletteState {
    pub active: bool,
    pub input: String,
    pub completions: Vec<String>,   // current candidates
    pub completion_idx: Option<usize>, // Tab-cycle cursor into completions
}
```

Add two methods:
```rust
impl CommandPaletteState {
    /// Advance to next completion and write it into `input`.
    pub fn cycle_next(&mut self);
    /// Step back to previous completion.
    pub fn cycle_prev(&mut self);
}
```
`cycle_next` sets `input` to `completions[idx]` and advances `idx` (wrapping).  
Reset `completion_idx` to `None` whenever `input` is mutated by a keystroke (not by a cycle).

---

### Input changes: `tui/input.rs`

Add two `TuiAction` variants:
```rust
TuiAction::CompleteNext,   // Tab
TuiAction::CompletePrev,   // Shift+Tab
```

In `handle_key`, inside the `palette.active` branch, add before the catch-all:
```rust
KeyCode::Tab => Some(TuiAction::CompleteNext),
KeyCode::BackTab => Some(TuiAction::CompletePrev),  // crossterm BackTab = Shift+Tab
```

---

### App changes: `tui/app.rs`

After every action that mutates `input` (`PaletteChar`, `PaletteBackspace`, `OpenPalette`), recompute:
```rust
model.command_palette.completions =
    completion::compute(&model.command_palette.input, &model);
model.command_palette.completion_idx = None;
```

Handle the two new actions:
```rust
TuiAction::CompleteNext => model.command_palette.cycle_next(),
TuiAction::CompletePrev => model.command_palette.cycle_prev(),
```
Both trigger a redraw.

On `ClosePalette`, clear completions along with input.

---

### Widget changes: `tui/widgets/command_palette.rs`

When `palette.active && !completions.is_empty()`, render a third line of suggestion chips directly
below the prompt. Each chip is `[ /scene ]`; the currently selected chip (if any) is highlighted
cyan/bold; others are DarkGray:

```
> /scene m█
  [/scene main] [/scene cam] [/scene brb]
```

If `completions` is empty, the third line is either blank or shows a dim "no completions" hint.

The command palette block height in `tui/layout.rs` should be at least **4 lines** to accommodate:
- Line 1: last result / status
- Line 2: input prompt
- Line 3: completion chips
- Border

Adjust `layout::compute` to allocate 4 lines minimum for the palette area instead of the current 3.

---

### Priority: H (user-visible feature, self-contained, well-scoped)
Files touched: `tui/completion.rs` (new), `tui/model.rs`, `tui/input.rs`, `tui/app.rs`,
`tui/widgets/command_palette.rs`, `tui/layout.rs`.  
No server-side or IPC changes required — all data comes from the existing `TuiModel` snapshot.

---

## Summary by priority

| Pri | Count | Theme |
|-----|-------|-------|
| H   | 3     | Duplication (session forwarder, response extractor, alias entries) |
| H   | 1     | Architecture (action dispatch in run_loop) |
| H   | 1     | Feature — command palette autocompletion (Tab/Shift+Tab, two-phase) |
| M   | 12    | Duplication, idioms, organisation, error handling, tests |
| L   | 8     | Polish, minor idioms, test coverage |

---

## Agent Loop Tasks

Implementation queue derived from the plan. Tasks are ordered by dependency and risk.
Full task details in `.agent-loop/tasks.json`.

| ID   | Title                                               | Type        | Pri | Status  |
|------|-----------------------------------------------------|-------------|-----|---------|
| T001 | Fix failing state_event_wire_json_is_stable test    | fix         | 1   | **done** |
| T002 | Extract spawn_session_forwarder in tui/app.rs       | improvement | 2   | **done** |
| T003 | Extract format_ipc_response in tui/app.rs           | improvement | 3   | **done** |
| T004 | Extract scene/audio alias entry helpers             | improvement | 4   | **done** |
| T005 | CHECKPOINT: build+test after H dedup refactors      | validation  | 5   | pending |
| T006 | Extract handle_action dispatch from run_loop        | improvement | 6   | pending |
| T007 | Fix double read-lock race in cmd_server_status      | fix         | 7   | pending |
| T008 | Remove/fix Command::Disconnect dead variant         | fix         | 8   | pending |
| T009 | Return error from cmd_dump_config when path is None | fix         | 9   | pending |
| T010 | Push connection error to model.logs                 | fix         | 10  | pending |
| T011 | CHECKPOINT: build+test after arch/error fixes       | validation  | 11  | pending |
| T012 | Merge validate_scene/audio_duplicates in dump.rs    | improvement | 12  | pending |
| T013 | Extract check_unique_aliases_shortcuts in schema.rs | improvement | 13  | pending |
| T014 | Add fn req/req_with helpers in obs/requests.rs      | improvement | 14  | pending |
| T015 | Fix required_string idiom (str::to_owned)           | improvement | 15  | pending |
| T016 | Fix redundant .clone() on HashSet inserts           | improvement | 16  | pending |
| T017 | Fix parser.rs clone()+clear() → mem::take          | improvement | 17  | pending |
| T018 | Eliminate TOPIC_*.to_string() hot-path allocs       | improvement | 18  | pending |
| T019 | Remove unnecessary level.clone() in cli/router.rs   | improvement | 19  | pending |
| T020 | Fix Span::raw clones in widgets/scenes+audio        | improvement | 20  | pending |
| T021 | Add tracing::debug for unhandled TOPIC_EVENTS vars  | improvement | 21  | pending |
| T022 | Define ES_* event subscription constants            | improvement | 22  | pending |
| T023 | CHECKPOINT: build+test+clippy after idioms          | validation  | 23  | pending |
| T024 | Reduce per-call allocation in TuiModel::scenes()    | improvement | 24  | pending |
| T025 | Introduce CommandExecutorConfig struct              | improvement | 25  | pending |
| T026 | Move render_unavailable to widgets module           | improvement | 26  | pending |
| T027 | Move MAX_TUI_LOG_ENTRIES to impl TuiModel const     | improvement | 27  | pending |
| T028 | CHECKPOINT: build+test after org changes            | validation  | 28  | pending |
| T029 | Fix variant exhaustiveness checks (compile-time)    | improvement | 29  | pending |
| T030 | Add tests for cmd_toggle_stream/toggle_record       | improvement | 30  | pending |
| T031 | Add test for TuiModel::scenes() hidden filter       | improvement | 31  | pending |
| T032 | Complete CommandPaletteState completions+cycling    | feature     | 32  | pending |
| T033 | Add TuiAction::CompleteNext/Prev + Tab keys         | feature     | 33  | pending |
| T034 | Create tui/completion.rs with compute fn            | feature     | 34  | pending |
| T035 | Wire completions into tui/app.rs event loop         | feature     | 35  | pending |
| T036 | Render completion chips in command_palette widget   | feature     | 36  | pending |
| T037 | FINAL VALIDATION: cargo check + test + clippy       | validation  | 37  | pending |
