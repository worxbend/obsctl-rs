use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::{
    keymap::{self, LEADER, Pending},
    model::{FocusPanel, SceneProfileEditor, SceneProfileStage, TuiModel, View},
};

/// Upper bound on a typed count prefix (`42j`). Vim has no limit; this one
/// exists so a leaned-on digit key can't grow an unbounded number.
pub const MAX_COUNT: u32 = 999;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiAction {
    Quit,
    /// `prefix: None` uses the configured `ui.command_palette_prefix`;
    /// `seed` is inserted after it (e.g. `"scene "` for `<leader>fs`).
    OpenPalette {
        prefix: Option<char>,
        seed: &'static str,
    },
    ClosePalette,
    PaletteChar(char),
    PaletteBackspace,
    /// Ctrl-U — wipe everything after the prefix, like vim's command line.
    PaletteClear,
    /// Ctrl-W — delete the word before the cursor.
    PaletteDeleteWord,
    PaletteSubmit,
    ReloadConfig,
    DumpConfig,
    ValidateConfig,
    ObsStatus,
    ServerStatus,
    ToggleStream,
    ToggleRecord,
    /// Reconnect the daemon's OBS WebSocket.
    ReconnectObs,
    /// Reconnect this TUI to the daemon.
    RetryConnect,
    // Panel focus
    FocusScenes,
    FocusAudio,
    FocusProfiles,
    FocusCollections,
    // Cross-panel navigation (Ctrl+arrows / Ctrl+hjkl), spatial across the
    // Scenes/Audio/Profiles/Collections 2x2 grid.
    FocusPaneLeft,
    FocusPaneRight,
    FocusPaneUp,
    FocusPaneDown,
    /// Tab / Shift-Tab — cycle panels in reading order.
    FocusPaneNext,
    FocusPanePrev,
    // Vertical navigation. The `usize` is the typed count prefix (1 by
    // default). What these move depends on the screen that is up — the
    // focused panel's list on the dashboard, the theme cursor in the settings
    // view — and that choice is made once, in `TuiModel::nav_up` and friends.
    NavUp(usize),
    NavDown(usize),
    NavTop,
    NavBottom,
    NavHalfPageUp,
    NavHalfPageDown,
    /// Enter — act on the focused row of the focused panel.
    Activate,
    /// Mouse: focus `panel` and move its cursor to `index`.
    SelectIndex(FocusPanel, usize),
    /// Mouse: [`SelectIndex`](TuiAction::SelectIndex) followed by an activate.
    ActivateIndex(FocusPanel, usize),
    // Audio actions
    ToggleMute,
    VolumeDown(usize),
    VolumeUp(usize),
    // Logs
    LogScrollUp(usize),
    LogScrollDown(usize),
    // Palette completion
    CompleteNext,
    CompletePrev,
    // Settings view
    OpenSettings,
    CloseSettings,
    /// Mouse: move the theme cursor straight to `index` and preview it.
    SettingsSelect(usize),
    ApplySettingsTheme,
    // Appearance toggles (`<leader>ui` / `<leader>ua`)
    ToggleIcons,
    ToggleAdvancedUi,
    // Scene-profile editor (`<leader>P`). A scene profile is a named set of
    // scene-visibility choices — nothing to do with an OBS profile, which is
    // what the Profiles panel and `FocusProfiles` above are about.
    OpenSceneProfiles,
    /// `P` on the dashboard (and `<leader>N`): switch to the next scene
    /// profile the config defines, passing through "no profile at all"
    /// between the last one and the first.
    SceneProfileCycleNext,
    CloseSceneProfiles,
    /// Everything the editor answers while it is open — see
    /// [`SceneProfileAction`].
    SceneProfile(SceneProfileAction),

    // Pending-key (vim sequence) bookkeeping
    SetPending(Pending),
    ClearPending,
    PushCount(u32),
}

/// What the scene-profile editor does with a key or a click. Every one of
/// these is only meaningful while the editor is open — that is, while
/// `TuiModel::scene_profile` is `Some` — which is why opening, closing and
/// cycling profiles stay on [`TuiAction`] itself: those three happen with no
/// editor on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneProfileAction {
    NavUp(usize),
    NavDown(usize),
    /// Mouse: move the editor's cursor to the row that was clicked.
    Select(usize),
    /// Enter on the picker: make a new scene profile, or edit the selected one.
    PickerConfirm,
    /// Switch to the selected scene profile and close the editor.
    Activate,
    /// Switch to no scene profile at all and close the editor.
    ClearActive,
    /// `d` on the picker: ask whether the selected profile should go. Sends
    /// nothing on its own — see [`SceneProfileAction::DeleteConfirm`].
    Delete,
    /// `y` (or `Enter`) on that question: send the delete.
    DeleteConfirm,
    /// `n`, `Esc`, or `q` on that question: leave the profile alone.
    DeleteCancel,
    ToggleHidden,
    BeginNaming,
    NameChar(char),
    NameBackspace,
    NameClear,
    NameDeleteWord,
    NameCommit,
    NameCancel,
    Save,
    /// Esc on the scene list: back to the picker, editor still open.
    Back,
}

pub fn handle_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    if model.command_palette.active {
        return palette_key(model, key);
    }
    // The scene-profile editor comes before the pending check, not after it:
    // its naming stage has to swallow `Space`, and `Space` is the leader key,
    // so leaving the pending machine in front would turn typing a name with a
    // space in it into a half-typed leader sequence. The price is that leader
    // sequences are inert while the editor is open, which is what a modal is.
    if let Some(editor) = model.scene_profile.as_ref() {
        return scene_profile_key(editor, key);
    }
    // A half-typed sequence is resolved before the per-screen bindings, so
    // every screen shares the one pending state machine in `keymap::resolve`.
    // The settings view used to carry its own inline copy of the `g` prefix,
    // which is how `gg` came to be spelled out twice.
    if model.pending.is_active() {
        return pending_key(model, key);
    }
    if model.view == View::Settings {
        return settings_key(model, key);
    }
    main_key(model, key)
}

fn palette_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => Some(TuiAction::ClosePalette),
        KeyCode::Char('c') if ctrl => Some(TuiAction::ClosePalette),
        KeyCode::Char('u') if ctrl => Some(TuiAction::PaletteClear),
        KeyCode::Char('w') if ctrl => Some(TuiAction::PaletteDeleteWord),
        KeyCode::Char('n') if ctrl => Some(TuiAction::CompleteNext),
        KeyCode::Char('p') if ctrl => Some(TuiAction::CompletePrev),
        KeyCode::Enter => Some(TuiAction::PaletteSubmit),
        // Backspacing over the prefix leaves the command line entirely, the
        // way `<BS>` on an empty vim `:` prompt does.
        KeyCode::Backspace if model.command_palette.input.chars().count() <= 1 => {
            Some(TuiAction::ClosePalette)
        }
        KeyCode::Backspace => Some(TuiAction::PaletteBackspace),
        KeyCode::Tab | KeyCode::Down => Some(TuiAction::CompleteNext),
        KeyCode::BackTab | KeyCode::Up => Some(TuiAction::CompletePrev),
        KeyCode::Char(c) => Some(TuiAction::PaletteChar(c)),
        _ => None,
    }
}

/// Keys of the scene-profile editor, which are the stage's alone: the modal
/// answers every key itself, so nothing typed at it can reach the dashboard
/// behind it.
///
/// The motions carry a count of 1 rather than the model's count prefix,
/// because no digit is bound here — a count typed at the dashboard is cleared
/// on the way in, and one typed at the modal never starts.
fn scene_profile_key(editor: &SceneProfileEditor, key: KeyEvent) -> Option<TuiAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Ctrl-C ends the program from here as it does from every other screen;
    // it is matched first so the plain `c` binding below cannot shadow it.
    if ctrl && key.code == KeyCode::Char('c') {
        return Some(TuiAction::Quit);
    }

    // A delete waiting to be confirmed owns the keyboard until it is answered.
    // Nothing else on the picker may run while the footer is asking a yes/no
    // question — an `a` typed at it would activate a profile the user believes
    // they are being asked about deleting.
    if editor.pending_delete.is_some() {
        return match key.code {
            KeyCode::Char('y') | KeyCode::Enter if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::DeleteConfirm))
            }
            KeyCode::Char('n') | KeyCode::Char('q') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::DeleteCancel))
            }
            KeyCode::Esc => Some(TuiAction::SceneProfile(SceneProfileAction::DeleteCancel)),
            // Every other key is ignored rather than treated as a "no": a
            // question this consequential is answered on purpose, and a
            // mistyped key that silently dismissed it would leave the user
            // unsure whether the profile had gone.
            _ => None,
        };
    }

    match editor.stage {
        SceneProfileStage::Picker => match key.code {
            KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NavDown(1)))
            }
            KeyCode::Up | KeyCode::Char('k') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NavUp(1)))
            }
            KeyCode::Enter => Some(TuiAction::SceneProfile(SceneProfileAction::PickerConfirm)),
            KeyCode::Char('a') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::Activate))
            }
            KeyCode::Char('c') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::ClearActive))
            }
            KeyCode::Char('d') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::Delete))
            }
            KeyCode::Esc | KeyCode::Char('q') => Some(TuiAction::CloseSceneProfiles),
            _ => None,
        },
        SceneProfileStage::Scenes => match key.code {
            KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NavDown(1)))
            }
            KeyCode::Up | KeyCode::Char('k') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NavUp(1)))
            }
            KeyCode::Char('t') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::ToggleHidden))
            }
            KeyCode::Char('n') if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::BeginNaming))
            }
            KeyCode::Enter => Some(TuiAction::SceneProfile(SceneProfileAction::Save)),
            KeyCode::Esc => Some(TuiAction::SceneProfile(SceneProfileAction::Back)),
            // `q` is deliberately unbound here. It closes the picker one stage
            // up, but on this stage there are unsaved toggles to lose, and
            // muscle memory should not be able to throw them away.
            _ => None,
        },
        SceneProfileStage::Naming => match key.code {
            KeyCode::Char('u') if ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NameClear))
            }
            KeyCode::Char('w') if ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NameDeleteWord))
            }
            KeyCode::Backspace => Some(TuiAction::SceneProfile(SceneProfileAction::NameBackspace)),
            KeyCode::Enter => Some(TuiAction::SceneProfile(SceneProfileAction::NameCommit)),
            KeyCode::Esc => Some(TuiAction::SceneProfile(SceneProfileAction::NameCancel)),
            // Space included: on this stage it is a character in a name, not
            // the leader key.
            KeyCode::Char(c) if !ctrl => {
                Some(TuiAction::SceneProfile(SceneProfileAction::NameChar(c)))
            }
            _ => None,
        },
    }
}

fn settings_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let count = model.count();
    match key.code {
        // Ctrl-C leaves the program from here as it does from the dashboard.
        // It used to be inert in this view, so a user who opened the theme
        // picker had no way out but Esc.
        KeyCode::Char('c') if ctrl => Some(TuiAction::Quit),
        KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => Some(TuiAction::CloseSettings),
        KeyCode::Char(d) if d.is_ascii_digit() => count_digit(model, d),
        KeyCode::Enter => Some(TuiAction::ApplySettingsTheme),
        // Everything else this view moves — `k`/`j`, `gg`, `G`, `Home`/`End`,
        // `PgUp`/`PgDn`, `Ctrl-U`/`Ctrl-D` — is the same motion vocabulary the
        // dashboard uses, resolved by the one function below.
        code => motion_key(code, ctrl, count),
    }
}

/// The motions every screen shares, in one place so a key bound here works
/// wherever there is something to move.
///
/// The plain (unmodified) motions are guarded on `!ctrl` because the
/// dashboard gives `Ctrl-hjkl` and `Ctrl`+arrows to cross-panel focus, and a
/// modifier must never fall through to the unmodified binding on the same key.
/// Sideways keys are absent on purpose: `h`/`l` mean "previous/next channel
/// strip" in the audio matrix and nothing anywhere else, so
/// [`main_key`] handles them itself.
fn motion_key(code: KeyCode, ctrl: bool, count: usize) -> Option<TuiAction> {
    match code {
        KeyCode::Char('d') if ctrl => Some(TuiAction::NavHalfPageDown),
        KeyCode::Char('u') if ctrl => Some(TuiAction::NavHalfPageUp),
        KeyCode::Up | KeyCode::Char('k') if !ctrl => Some(TuiAction::NavUp(count)),
        KeyCode::Down | KeyCode::Char('j') if !ctrl => Some(TuiAction::NavDown(count)),
        KeyCode::PageUp => Some(TuiAction::NavHalfPageUp),
        KeyCode::PageDown => Some(TuiAction::NavHalfPageDown),
        KeyCode::Char('g') if !ctrl => Some(TuiAction::SetPending(Pending::G)),
        KeyCode::Char('G') if !ctrl => Some(TuiAction::NavBottom),
        // `End`, `Home` and the page keys are left unguarded: nothing binds a
        // Ctrl-modified version of them, so there is no binding to shadow.
        KeyCode::End => Some(TuiAction::NavBottom),
        KeyCode::Home => Some(TuiAction::NavTop),
        _ => None,
    }
}

/// Second key of a pending sequence. Anything unmapped dismisses the
/// which-key overlay instead of falling through to the normal bindings —
/// otherwise a typo mid-sequence would fire an unrelated command.
fn pending_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(keymap::resolve(model.pending, c).unwrap_or(TuiAction::ClearPending))
        }
        _ => Some(TuiAction::ClearPending),
    }
}

fn main_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let count = model.count();

    match key.code {
        // Ctrl-modified bindings are matched first so the modifier always
        // wins over the plain binding on the same key.
        KeyCode::Char('c') if ctrl => Some(TuiAction::Quit),
        KeyCode::Char('t') if ctrl => Some(TuiAction::OpenSettings),
        KeyCode::Left | KeyCode::Char('h') if ctrl => Some(TuiAction::FocusPaneLeft),
        KeyCode::Right | KeyCode::Char('l') if ctrl => Some(TuiAction::FocusPaneRight),
        KeyCode::Up | KeyCode::Char('k') if ctrl => Some(TuiAction::FocusPaneUp),
        KeyCode::Down | KeyCode::Char('j') if ctrl => Some(TuiAction::FocusPaneDown),

        KeyCode::F(2) => Some(TuiAction::OpenSettings),
        KeyCode::Esc => Some(TuiAction::ClearPending),
        KeyCode::Tab => Some(TuiAction::FocusPaneNext),
        KeyCode::BackTab => Some(TuiAction::FocusPanePrev),

        // Command line. `:` is the vim prompt; `/` stays mapped as a legacy
        // alias and inserts itself, so both keep working verbatim.
        KeyCode::Char(':') => Some(TuiAction::OpenPalette {
            prefix: Some(':'),
            seed: "",
        }),
        KeyCode::Char('/') => Some(TuiAction::OpenPalette {
            prefix: Some('/'),
            seed: "",
        }),

        // Pending sequences. `g` is a prefix too, but it is one of the shared
        // motions, so `motion_key` opens it below.
        KeyCode::Char(LEADER) => Some(TuiAction::SetPending(Pending::Leader)),
        KeyCode::Char(d) if d.is_ascii_digit() => count_digit(model, d),

        KeyCode::Char('q') => Some(TuiAction::Quit),
        KeyCode::Char('r') => Some(TuiAction::ReloadConfig),
        KeyCode::Char('D') => Some(TuiAction::DumpConfig),
        KeyCode::Char('R') => Some(TuiAction::RetryConnect),

        // Panel focus
        KeyCode::Char('s') => Some(TuiAction::FocusScenes),
        KeyCode::Char('a') => Some(TuiAction::FocusAudio),
        KeyCode::Char('p') => Some(TuiAction::FocusProfiles),
        KeyCode::Char('c') => Some(TuiAction::FocusCollections),

        // Scene profiles. Upper case, because the lower-case `p` above focuses
        // the Profiles panel and an OBS profile is a different thing entirely.
        // Every other route to switching scene profiles — the modal, the
        // palette — costs several keystrokes and a typed name, which is what
        // made a feature whose whole point is flipping between two scene
        // layouts feel like it had no switch.
        KeyCode::Char('P') => Some(TuiAction::SceneProfileCycleNext),

        // Audio-panel actions. The audio matrix draws its inputs as vertical
        // channel strips laid out left to right, so its axes are the other
        // way round from the list panels: sideways moves between inputs and
        // up/down rides the fader, matching the direction of the fader on
        // screen. These have to be matched before the generic motions below,
        // which would otherwise claim the same keys.
        KeyCode::Char('m') if model.focus == FocusPanel::Audio => Some(TuiAction::ToggleMute),
        KeyCode::Left | KeyCode::Char('h') if model.focus == FocusPanel::Audio => {
            Some(TuiAction::NavUp(count))
        }
        KeyCode::Right | KeyCode::Char('l') if model.focus == FocusPanel::Audio => {
            Some(TuiAction::NavDown(count))
        }
        KeyCode::Up | KeyCode::Char('k') if model.focus == FocusPanel::Audio => {
            Some(TuiAction::VolumeUp(count))
        }
        KeyCode::Down | KeyCode::Char('j') if model.focus == FocusPanel::Audio => {
            Some(TuiAction::VolumeDown(count))
        }

        KeyCode::Enter => Some(TuiAction::Activate),

        // Motions, shared with the settings view.
        code => motion_key(code, ctrl, count),
    }
}

/// `0` only extends an existing count — on its own it is vim's
/// start-of-line motion, which this UI has nothing to do with.
fn count_digit(model: &TuiModel, digit: char) -> Option<TuiAction> {
    let digit = digit.to_digit(10)?;
    if digit == 0 && model.pending_count.is_none() {
        return None;
    }
    Some(TuiAction::PushCount(digit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    #[test]
    fn ctrl_arrows_navigate_between_panes() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Right)),
            Some(TuiAction::FocusPaneRight)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Left)),
            Some(TuiAction::FocusPaneLeft)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Down)),
            Some(TuiAction::FocusPaneDown)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Up)),
            Some(TuiAction::FocusPaneUp)
        );
    }

    #[test]
    fn ctrl_vim_keys_navigate_between_panes() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('l'))),
            Some(TuiAction::FocusPaneRight)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('h'))),
            Some(TuiAction::FocusPaneLeft)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('j'))),
            Some(TuiAction::FocusPaneDown)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('k'))),
            Some(TuiAction::FocusPaneUp)
        );
    }

    #[test]
    fn ctrl_pane_navigation_takes_priority_over_audio_panel_keys() {
        let mut model = TuiModel::default();
        model.focus = FocusPanel::Audio;
        assert_eq!(
            handle_key(&model, key(KeyCode::Left)),
            Some(TuiAction::NavUp(1))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Down)),
            Some(TuiAction::VolumeDown(1))
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Left)),
            Some(TuiAction::FocusPaneLeft)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('h'))),
            Some(TuiAction::FocusPaneLeft)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('j'))),
            Some(TuiAction::FocusPaneDown)
        );
    }

    #[test]
    fn a_ctrl_modified_g_is_not_a_motion() {
        // `g` opens the which-key menu and `G` jumps to the bottom, but only
        // unmodified: a modifier must never fall through to the plain
        // binding, the way `Ctrl-k` does not mean "move up".
        let model = TuiModel::default();
        assert_eq!(handle_key(&model, ctrl_key(KeyCode::Char('g'))), None);
        assert_eq!(handle_key(&model, ctrl_key(KeyCode::Char('G'))), None);
    }

    #[test]
    fn the_audio_matrix_swaps_the_navigation_axes() {
        let mut model = TuiModel::default();
        model.focus = FocusPanel::Audio;
        // Sideways moves between channel strips...
        for k in [KeyCode::Left, KeyCode::Char('h')] {
            assert_eq!(handle_key(&model, key(k)), Some(TuiAction::NavUp(1)));
        }
        for k in [KeyCode::Right, KeyCode::Char('l')] {
            assert_eq!(handle_key(&model, key(k)), Some(TuiAction::NavDown(1)));
        }
        // ...and up/down rides the fader of the selected one.
        for k in [KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(handle_key(&model, key(k)), Some(TuiAction::VolumeUp(1)));
        }
        for k in [KeyCode::Down, KeyCode::Char('j')] {
            assert_eq!(handle_key(&model, key(k)), Some(TuiAction::VolumeDown(1)));
        }

        // Every other panel keeps up/down as its list motion.
        model.focus = FocusPanel::Scenes;
        assert_eq!(
            handle_key(&model, key(KeyCode::Down)),
            Some(TuiAction::NavDown(1))
        );
        assert_eq!(handle_key(&model, key(KeyCode::Left)), None);
    }

    #[test]
    fn tab_and_shift_tab_cycle_panels() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, key(KeyCode::Tab)),
            Some(TuiAction::FocusPaneNext)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::BackTab)),
            Some(TuiAction::FocusPanePrev)
        );
    }

    #[test]
    fn colon_opens_the_palette_with_a_colon_and_slash_with_a_slash() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ch(':')),
            Some(TuiAction::OpenPalette {
                prefix: Some(':'),
                seed: ""
            })
        );
        assert_eq!(
            handle_key(&model, ch('/')),
            Some(TuiAction::OpenPalette {
                prefix: Some('/'),
                seed: ""
            })
        );
    }

    #[test]
    fn space_opens_the_leader_menu_and_a_second_key_resolves_within_it() {
        let mut model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ch(LEADER)),
            Some(TuiAction::SetPending(Pending::Leader))
        );

        model.pending = Pending::Leader;
        assert_eq!(
            handle_key(&model, ch('s')),
            Some(TuiAction::SetPending(Pending::LeaderGroup('s')))
        );

        model.pending = Pending::LeaderGroup('s');
        assert_eq!(handle_key(&model, ch('s')), Some(TuiAction::ToggleStream));
    }

    #[test]
    fn leader_shadows_the_plain_binding_on_the_same_key() {
        // Bare `s` focuses the scenes panel; `<leader>s` must open the stream
        // group instead of focusing anything.
        let mut model = TuiModel::default();
        assert_eq!(handle_key(&model, ch('s')), Some(TuiAction::FocusScenes));
        model.pending = Pending::Leader;
        assert_eq!(
            handle_key(&model, ch('s')),
            Some(TuiAction::SetPending(Pending::LeaderGroup('s')))
        );
    }

    #[test]
    fn an_unmapped_key_mid_sequence_dismisses_instead_of_firing_a_command() {
        let mut model = TuiModel::default();
        model.pending = Pending::Leader;
        // `q` quits from the main view, but here it is `<leader>q`.
        assert_eq!(handle_key(&model, ch('q')), Some(TuiAction::Quit));
        // `z` is mapped nowhere in the leader menu.
        assert_eq!(handle_key(&model, ch('z')), Some(TuiAction::ClearPending));
        assert_eq!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::ClearPending)
        );
    }

    #[test]
    fn gg_and_shift_g_jump_to_the_ends_of_the_list() {
        let mut model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ch('g')),
            Some(TuiAction::SetPending(Pending::G))
        );
        model.pending = Pending::G;
        assert_eq!(handle_key(&model, ch('g')), Some(TuiAction::NavTop));

        model.pending = Pending::None;
        assert_eq!(handle_key(&model, ch('G')), Some(TuiAction::NavBottom));
    }

    #[test]
    fn digits_build_a_count_prefix_that_scales_the_next_motion() {
        let mut model = TuiModel::default();
        // A leading zero is not a count.
        assert_eq!(handle_key(&model, ch('0')), None);
        assert_eq!(handle_key(&model, ch('1')), Some(TuiAction::PushCount(1)));

        model.pending_count = Some(1);
        assert_eq!(handle_key(&model, ch('0')), Some(TuiAction::PushCount(0)));

        model.pending_count = Some(10);
        assert_eq!(handle_key(&model, ch('j')), Some(TuiAction::NavDown(10)));
        assert_eq!(handle_key(&model, ch('k')), Some(TuiAction::NavUp(10)));
    }

    #[test]
    fn count_scales_volume_nudges_too() {
        let mut model = TuiModel::default();
        model.focus = FocusPanel::Audio;
        model.pending_count = Some(3);
        assert_eq!(handle_key(&model, ch('k')), Some(TuiAction::VolumeUp(3)));
        assert_eq!(handle_key(&model, ch('j')), Some(TuiAction::VolumeDown(3)));
        // The count scales the sideways strip motion just the same.
        assert_eq!(handle_key(&model, ch('l')), Some(TuiAction::NavDown(3)));
        assert_eq!(handle_key(&model, ch('h')), Some(TuiAction::NavUp(3)));
    }

    #[test]
    fn ctrl_d_and_ctrl_u_scroll_by_half_a_pane() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('d'))),
            Some(TuiAction::NavHalfPageDown)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('u'))),
            Some(TuiAction::NavHalfPageUp)
        );
    }

    #[test]
    fn ctrl_t_opens_settings_from_main_view() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('t'))),
            Some(TuiAction::OpenSettings)
        );
    }

    #[test]
    fn f2_opens_settings_from_main_view() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, key(KeyCode::F(2))),
            Some(TuiAction::OpenSettings)
        );
    }

    #[test]
    fn settings_view_intercepts_navigation_and_close_keys() {
        let mut model = TuiModel::default();
        model.view = View::Settings;

        assert_eq!(
            handle_key(&model, key(KeyCode::Down)),
            Some(TuiAction::NavDown(1))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Up)),
            Some(TuiAction::NavUp(1))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Enter)),
            Some(TuiAction::ApplySettingsTheme)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::CloseSettings)
        );
        // 'q' closes settings rather than quitting the whole app.
        assert_eq!(handle_key(&model, ch('q')), Some(TuiAction::CloseSettings));

        // Sanity: KeyEventKind default is Press, matching the harness filter.
        assert_eq!(key(KeyCode::Esc).kind, KeyEventKind::Press);
    }

    #[test]
    fn settings_view_supports_vim_motions_and_counts() {
        let mut model = TuiModel::default();
        model.view = View::Settings;

        assert_eq!(
            handle_key(&model, ch('g')),
            Some(TuiAction::SetPending(Pending::G))
        );
        model.pending = Pending::G;
        assert_eq!(handle_key(&model, ch('g')), Some(TuiAction::NavTop));

        model.pending = Pending::None;
        assert_eq!(handle_key(&model, ch('G')), Some(TuiAction::NavBottom));

        model.pending_count = Some(5);
        assert_eq!(handle_key(&model, ch('j')), Some(TuiAction::NavDown(5)));
    }

    /// The settings view used to reach `_ => None` for all of these, so the
    /// page keys did nothing there and — worse — Ctrl-C could not end the
    /// program once the theme picker was open.
    #[test]
    fn settings_view_answers_the_same_page_and_quit_keys_as_the_dashboard() {
        let mut model = TuiModel::default();
        model.view = View::Settings;

        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('c'))),
            Some(TuiAction::Quit)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::PageDown)),
            Some(TuiAction::NavHalfPageDown)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::PageUp)),
            Some(TuiAction::NavHalfPageUp)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('d'))),
            Some(TuiAction::NavHalfPageDown)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('u'))),
            Some(TuiAction::NavHalfPageUp)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Home)),
            Some(TuiAction::NavTop)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::End)),
            Some(TuiAction::NavBottom)
        );
    }

    /// The `g` prefix is resolved by `keymap::resolve` on every screen, so the
    /// settings view cannot drift away from what `gg` means elsewhere.
    #[test]
    fn a_pending_sequence_resolves_the_same_way_in_the_settings_view() {
        let mut model = TuiModel::default();
        model.view = View::Settings;
        model.pending = Pending::G;

        assert_eq!(handle_key(&model, ch('g')), Some(TuiAction::NavTop));
        // An unmapped second key dismisses the overlay rather than firing
        // whatever that key means on its own.
        assert_eq!(handle_key(&model, ch('z')), Some(TuiAction::ClearPending));
        assert_eq!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::ClearPending)
        );
    }

    #[test]
    fn palette_editing_keys_are_vim_flavoured() {
        let mut model = TuiModel::default();
        model.command_palette.active = true;
        model.command_palette.input = ":scene Main".to_string();

        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('u'))),
            Some(TuiAction::PaletteClear)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('w'))),
            Some(TuiAction::PaletteDeleteWord)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('n'))),
            Some(TuiAction::CompleteNext)
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('p'))),
            Some(TuiAction::CompletePrev)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Down)),
            Some(TuiAction::CompleteNext)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Backspace)),
            Some(TuiAction::PaletteBackspace)
        );
    }

    #[test]
    fn backspacing_over_the_prompt_prefix_closes_the_palette() {
        let mut model = TuiModel::default();
        model.command_palette.active = true;
        model.command_palette.input = ":".to_string();
        assert_eq!(
            handle_key(&model, key(KeyCode::Backspace)),
            Some(TuiAction::ClosePalette)
        );
    }

    /// `P` and `p` are one Shift apart and mean unrelated things — cycling
    /// scene profiles versus focusing the Profiles panel, which lists OBS
    /// profiles. A binding added on the wrong case would be caught here
    /// rather than by a user whose scene list changed when they meant to
    /// change panels.
    #[test]
    fn shift_p_cycles_scene_profiles_while_plain_p_focuses_the_profiles_panel() {
        let model = TuiModel::default();
        assert_eq!(
            handle_key(&model, ch('P')),
            Some(TuiAction::SceneProfileCycleNext)
        );
        assert_eq!(handle_key(&model, ch('p')), Some(TuiAction::FocusProfiles));
        // And the modal still opens on the leader sequence, unchanged.
        let mut pending = TuiModel::default();
        pending.pending = Pending::Leader;
        assert_eq!(
            handle_key(&pending, ch('P')),
            Some(TuiAction::OpenSceneProfiles)
        );
        assert_eq!(
            handle_key(&pending, ch('N')),
            Some(TuiAction::SceneProfileCycleNext),
            "and <leader>N is the same cycle the bare P runs"
        );
    }

    // --- the scene-profile editor ---

    /// A model with the editor open on `stage`, reached the way a user reaches
    /// it: `<leader>P`, then Enter on the "new scene profile" row, then a
    /// typed name.
    fn editor_on(stage: SceneProfileStage) -> TuiModel {
        let mut model = TuiModel::default();
        model.open_scene_profiles();
        if stage != SceneProfileStage::Picker {
            model.scene_profile_confirm_picker();
        }
        if stage == SceneProfileStage::Scenes {
            model.scene_profile_edit_name(|name| name.push('a'));
            assert!(model.scene_profile_commit_name().is_ok());
        }
        assert_eq!(model.scene_profile.as_ref().unwrap().stage, stage);
        model
    }

    /// The whole reason the editor is consulted before the pending state
    /// machine: `Space` is the leader key, and a scene profile called
    /// "late night" has one in the middle of it.
    #[test]
    fn space_types_a_character_while_naming_instead_of_opening_the_leader_menu() {
        let model = editor_on(SceneProfileStage::Naming);
        assert_eq!(
            handle_key(&model, ch(LEADER)),
            Some(TuiAction::SceneProfile(SceneProfileAction::NameChar(' ')))
        );
        assert_eq!(
            handle_key(&model, ch('n')),
            Some(TuiAction::SceneProfile(SceneProfileAction::NameChar('n')))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Enter)),
            Some(TuiAction::SceneProfile(SceneProfileAction::NameCommit))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::SceneProfile(SceneProfileAction::NameCancel))
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('u'))),
            Some(TuiAction::SceneProfile(SceneProfileAction::NameClear))
        );
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('w'))),
            Some(TuiAction::SceneProfile(SceneProfileAction::NameDeleteWord))
        );
    }

    /// A half-typed leader sequence does not survive the editor opening, and
    /// while the editor is up the pending machine never gets the key.
    #[test]
    fn the_editor_answers_keys_before_a_pending_sequence_does() {
        let mut model = editor_on(SceneProfileStage::Picker);
        model.pending = Pending::Leader;
        assert_eq!(
            handle_key(&model, ch('s')),
            None,
            "`<leader>s` would open the stream group on the dashboard"
        );
        assert_eq!(
            handle_key(&model, ch('j')),
            Some(TuiAction::SceneProfile(SceneProfileAction::NavDown(1)))
        );
    }

    /// The palette is checked first, so a command line opened over the editor
    /// still takes what is typed at it.
    #[test]
    fn the_palette_still_wins_over_the_editor() {
        let mut model = editor_on(SceneProfileStage::Picker);
        model.command_palette.active = true;
        model.command_palette.input = ":sc".to_string();
        assert_eq!(
            handle_key(&model, ch('e')),
            Some(TuiAction::PaletteChar('e'))
        );
    }

    #[test]
    fn the_picker_activates_clears_deletes_and_closes() {
        let model = editor_on(SceneProfileStage::Picker);
        assert_eq!(
            handle_key(&model, key(KeyCode::Enter)),
            Some(TuiAction::SceneProfile(SceneProfileAction::PickerConfirm))
        );
        assert_eq!(
            handle_key(&model, ch('a')),
            Some(TuiAction::SceneProfile(SceneProfileAction::Activate))
        );
        assert_eq!(
            handle_key(&model, ch('c')),
            Some(TuiAction::SceneProfile(SceneProfileAction::ClearActive))
        );
        assert_eq!(
            handle_key(&model, ch('d')),
            Some(TuiAction::SceneProfile(SceneProfileAction::Delete))
        );
        assert_eq!(
            handle_key(&model, ch('q')),
            Some(TuiAction::CloseSceneProfiles)
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::CloseSceneProfiles)
        );
        // Ctrl-C still ends the program, as it does from every other screen.
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('c'))),
            Some(TuiAction::Quit)
        );
    }

    /// While a delete is waiting to be confirmed the picker's own keys are
    /// off. `a` is the neighbour of `d`, and a user who has just been asked
    /// "delete streaming?" must not be able to answer it by switching a
    /// profile on.
    #[test]
    fn a_pending_delete_takes_every_key_until_it_is_answered() {
        let mut model = editor_on(SceneProfileStage::Picker);
        // Set straight onto the editor: which profile is armed is the model's
        // business, and this layer only asks whether a question is up.
        model.scene_profile.as_mut().unwrap().pending_delete = Some("streaming".to_string());

        assert_eq!(
            handle_key(&model, ch('y')),
            Some(TuiAction::SceneProfile(SceneProfileAction::DeleteConfirm))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Enter)),
            Some(TuiAction::SceneProfile(SceneProfileAction::DeleteConfirm))
        );
        for cancel in [ch('n'), ch('q'), key(KeyCode::Esc)] {
            assert_eq!(
                handle_key(&model, cancel),
                Some(TuiAction::SceneProfile(SceneProfileAction::DeleteCancel))
            );
        }
        // Not an answer, and not a way past the question either.
        assert_eq!(handle_key(&model, ch('a')), None);
        assert_eq!(handle_key(&model, ch('j')), None);
        // Ctrl-C is still the way out of the program.
        assert_eq!(
            handle_key(&model, ctrl_key(KeyCode::Char('c'))),
            Some(TuiAction::Quit)
        );
    }

    #[test]
    fn the_scene_stage_toggles_renames_saves_and_goes_back() {
        let model = editor_on(SceneProfileStage::Scenes);
        assert_eq!(
            handle_key(&model, ch('t')),
            Some(TuiAction::SceneProfile(SceneProfileAction::ToggleHidden))
        );
        assert_eq!(
            handle_key(&model, ch('n')),
            Some(TuiAction::SceneProfile(SceneProfileAction::BeginNaming))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Enter)),
            Some(TuiAction::SceneProfile(SceneProfileAction::Save))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::SceneProfile(SceneProfileAction::Back))
        );
        // `q` is unbound here: there are unsaved toggles to lose.
        assert_eq!(handle_key(&model, ch('q')), None);
    }

    #[test]
    fn leader_is_inert_while_the_palette_is_open() {
        let mut model = TuiModel::default();
        model.command_palette.active = true;
        model.command_palette.input = ":scene".to_string();
        assert_eq!(
            handle_key(&model, ch(LEADER)),
            Some(TuiAction::PaletteChar(' '))
        );
    }
}
