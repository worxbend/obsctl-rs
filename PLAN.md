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

### M — `config/dump.rs`: near-identical duplicate validation fns [DONE T012]
`validate_scene_duplicates` and `validate_audio_duplicates` (lines 138–180) share 95% of their
body. Merge into a single generic:
```rust
fn validate_no_duplicates(
    label: &str,
    items: impl Iterator<Item = (Option<&str>, Option<&str>)>, // (alias, shortcut)
) -> Result<()>
```

### M — `config/schema.rs`: same alias/shortcut uniqueness loop duplicated for scenes and audio [DONE T013]
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

The command palette block height in `tui/layout.rs` should be at least **5 terminal rows** to accommodate:
- Line 1: last result / status
- Line 2: input prompt
- Line 3: completion chips
- Top/bottom border

Adjust `layout::compute` to allocate enough lines for the palette area.

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

Implementation queue derived from the plan and reconciled with the current codebase.
Tasks are ordered by dependency, correctness risk, and user value. Full task details live in
`.agent-loop/tasks.json`.

| ID   | Title                                      | Type        | Pri | Status |
|------|--------------------------------------------|-------------|-----|--------|
| T001 | Fix state event wire JSON test             | fix         | 1   | done |
| T002 | Extract TUI session forwarder              | improvement | 2   | done |
| T003 | Extract IPC response formatter             | improvement | 3   | done |
| T004 | Extract alias entry helpers                | improvement | 4   | done |
| T005 | Checkpoint H dedup refactors               | validation  | 5   | done |
| T006 | Extract TUI action dispatcher              | improvement | 6   | done |
| T007 | Fix server-status read race                | fix         | 7   | done |
| T008 | Remove dead disconnect command             | fix         | 8   | done |
| T009 | Error when dump config has no path         | fix         | 9   | done |
| T010 | Persist TUI connection error               | fix         | 10  | done |
| T011 | Checkpoint architecture fixes              | validation  | 11  | done |
| T012 | Merge dump duplicate validation            | improvement | 12  | done |
| T013 | Extract schema uniqueness helper           | improvement | 13  | done |
| T014 | Add OBS request helpers                    | improvement | 14  | done |
| T015 | Use str::to_owned in required_string       | improvement | 15  | done |
| T016 | Remove redundant HashSet clones            | improvement | 16  | done |
| T017 | Use mem::take in parser                    | improvement | 17  | done |
| T018 | Eliminate topic string allocations         | improvement | 18  | done |
| T019 | Remove router level clone                  | improvement | 19  | done |
| T020 | Avoid widget text clones                   | improvement | 20  | done |
| T021 | Log unhandled OBS event variants           | improvement | 21  | done |
| T022 | Name event subscription bits               | improvement | 22  | done |
| T023 | Checkpoint idiom refactors                 | validation  | 23  | done |
| T024 | Cache visible scenes                       | improvement | 24  | done |
| T025 | Add command executor config                | improvement | 25  | done |
| T026 | Move unavailable widget                    | improvement | 26  | done |
| T027 | Move max log constant                      | improvement | 27  | done |
| T028 | Checkpoint organization changes            | validation  | 28  | done |
| T029 | Add variant exhaustiveness checks          | improvement | 29  | done |
| T030 | Test stream and record toggles             | improvement | 30  | done |
| T031 | Test hidden scene filtering                | improvement | 31  | done |
| T032 | Add palette completion state               | feature     | 32  | done |
| T033 | Add completion key actions                 | feature     | 33  | done |
| T034 | Create palette completion engine           | feature     | 34  | done |
| T035 | Wire completions into TUI app              | feature     | 35  | done |
| T038 | Fix completion coverage and ordering       | improvement | 36  | done |
| T036 | Render inline completion chips             | feature     | 37  | done |
| T040 | Match completion argument commands case-insensitively | improvement | 38  | done |
| T039 | Add completion widget tests                | improvement | 39  | pending |
| T041 | Complete parser scene/audio aliases        | improvement | 40  | pending |
| T037 | Run final validation                       | validation  | 41  | pending |

## Plan Expansion Log

- Initial queue decisions: correctness and failing/regression work was ordered first, followed by high-priority duplication refactors, architecture/error-handling work, medium idiom/organization cleanup, test-quality work, then the command-palette completion feature.
- 2026-07-02 analysis reconciliation: `.agent-loop/tasks.json` was normalized to the required schema and existing completed records were preserved. `T035` was marked done after source inspection showed completion recomputation/clearing/cycling was already wired in `src/tui/app.rs`; `cargo check` passed.
- Newly queued `T038` (discovered): completion currently omits the supported `/help` command and does not explicitly enforce the plan's exact-match-before-alphabetical ordering. This should be fixed before final rendering validation.
- Newly queued `T039` (discovered): existing TUI widget tests cover the command palette prompt and last result, but not completion chips or the empty completion hint. Add focused rendering tests after `T036`.
- 2026-07-02 implementation T038: `src/tui/completion.rs` now includes `/help`, sorts case-insensitive exact matches before other prefix matches, and has focused unit tests for `/help` plus command/argument exact ordering. `cargo fmt --check`, `cargo check`, `cargo check --all-targets --all-features`, and `cargo test tui::completion` pass after applying rustfmt. Newly queued `T040` for the adjacent parser-consistency gap where argument completion dispatch is still case-sensitive after a space; final validation now depends on it.
- 2026-07-02 implementation T036: `src/tui/widgets/command_palette.rs` now renders inline bracketed completion chips below the prompt, uses cyan bold styling for the selected chip, DarkGray for unselected chips, and a dim `no completions` hint when active with no candidates. The floating `Clear`/`List` popup path was removed. `src/tui/layout.rs` now allocates 5 rows to the palette because Ratatui borders leave three inner rows for status, prompt, and completions. `cargo fmt --check`, `cargo check`, `cargo test --test tui_widget_rendering command_palette`, and `cargo check --all-targets --all-features` pass after applying rustfmt. Plan expansion added no new tasks; existing `T039` was updated for the 5-line palette area, with `T040` and `T037` still pending.
- 2026-07-02 validation VALIDATION-50: inspected Cargo metadata, README Development, and `.agent-loop/config.json`; `cargo fmt --check`, `cargo check --all-targets --all-features`, `cargo test --all-targets --all-features`, `cargo clippy --all-targets --all-features -- -D warnings`, and README-listed `cargo build --release` all pass. No in-scope fixes were needed. Plan expansion added no new tasks; existing `T040` remains the parser-consistency correctness fix, existing `T039` remains the completion widget coverage task, and final validation `T037` remains pending until both are complete. Pending priorities were adjusted so `T040` runs before `T039`.
- 2026-07-02 implementation T040: `src/tui/completion.rs` now matches argument-completion command dispatch case-insensitively after the first space and preserves the typed command token in generated scene/audio suggestions. Independent review found the plan assumption was stale: `src/domain/parser.rs` was still matching command names case-sensitively, so selecting preserved uppercase suggestions would have failed. The parser now normalizes only the command token for dispatch while preserving arguments and unknown-command error text. Focused tests cover `/SCENE m`, `/MUTE mic`, and parser uppercase command names. `cargo fmt --check`, `cargo check`, `cargo test tui::completion`, `cargo test domain::parser`, and `cargo check --all-targets --all-features` pass after applying rustfmt. Plan expansion queued `T041` for parser scene/audio alias argument completions (`/set-scene`, `/volume`); `T039` remains next, and final validation `T037` now depends on `T041`.
- Known follow-up intentionally left unqueued: the `state_store.rs` serialization concern appears already addressed for snapshot broadcasts by logging and returning on `serde_json::to_value` failure; no `serde_json::to_value(...).unwrap_or_default()` remains in `state_store.rs` or `build_snapshot`.
- Known follow-up intentionally left unqueued: the `config/model.rs` shared `ResourceMetadata` idea is a low-priority modeling refactor, not required for the remaining completion feature or correctness goals.
