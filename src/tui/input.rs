use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::{
    keymap::{self, LEADER, Pending},
    model::{FocusPanel, TuiModel, View},
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
    // List navigation. The `usize` is the typed count prefix (1 by default).
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
    SettingsNavUp(usize),
    SettingsNavDown(usize),
    SettingsNavTop,
    SettingsNavBottom,
    SettingsSelect(usize),
    ApplySettingsTheme,
    // Appearance toggles (`<leader>ui` / `<leader>ua`)
    ToggleIcons,
    ToggleAdvancedUi,
    // Pending-key (vim sequence) bookkeeping
    SetPending(Pending),
    ClearPending,
    PushCount(u32),
}

pub fn handle_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    if model.command_palette.active {
        return palette_key(model, key);
    }
    if model.view == View::Settings {
        return settings_key(model, key);
    }
    if model.pending.is_active() {
        return pending_key(model, key);
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

fn settings_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    if model.pending == Pending::G {
        return match key.code {
            KeyCode::Char('g') => Some(TuiAction::SettingsNavTop),
            _ => Some(TuiAction::ClearPending),
        };
    }

    let count = model.count();
    match key.code {
        KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => Some(TuiAction::CloseSettings),
        KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::SettingsNavUp(count)),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::SettingsNavDown(count)),
        KeyCode::Char('g') => Some(TuiAction::SetPending(Pending::G)),
        KeyCode::Char('G') | KeyCode::End => Some(TuiAction::SettingsNavBottom),
        KeyCode::Home => Some(TuiAction::SettingsNavTop),
        KeyCode::Char(d) if d.is_ascii_digit() => count_digit(model, d),
        KeyCode::Enter => Some(TuiAction::ApplySettingsTheme),
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
        KeyCode::Char('d') if ctrl => Some(TuiAction::NavHalfPageDown),
        KeyCode::Char('u') if ctrl => Some(TuiAction::NavHalfPageUp),
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

        // Pending sequences.
        KeyCode::Char(LEADER) => Some(TuiAction::SetPending(Pending::Leader)),
        KeyCode::Char('g') => Some(TuiAction::SetPending(Pending::G)),
        KeyCode::Char('G') => Some(TuiAction::NavBottom),
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

        // Motions
        KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::NavUp(count)),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::NavDown(count)),
        KeyCode::PageUp => Some(TuiAction::NavHalfPageUp),
        KeyCode::PageDown => Some(TuiAction::NavHalfPageDown),
        KeyCode::Home => Some(TuiAction::NavTop),
        KeyCode::End => Some(TuiAction::NavBottom),
        KeyCode::Enter => Some(TuiAction::Activate),

        _ => None,
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
            Some(TuiAction::SettingsNavDown(1))
        );
        assert_eq!(
            handle_key(&model, key(KeyCode::Up)),
            Some(TuiAction::SettingsNavUp(1))
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
        assert_eq!(handle_key(&model, ch('g')), Some(TuiAction::SettingsNavTop));

        model.pending = Pending::None;
        assert_eq!(
            handle_key(&model, ch('G')),
            Some(TuiAction::SettingsNavBottom)
        );

        model.pending_count = Some(5);
        assert_eq!(
            handle_key(&model, ch('j')),
            Some(TuiAction::SettingsNavDown(5))
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
