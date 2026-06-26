use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::model::{FocusPanel, TuiModel};

#[derive(Debug)]
pub enum TuiAction {
    Quit,
    OpenPalette,
    ClosePalette,
    PaletteChar(char),
    PaletteBackspace,
    PaletteSubmit,
    ReloadConfig,
    DumpConfig,
    RetryConnect,
    // Navigation
    FocusScenes,
    FocusAudio,
    NavUp,
    NavDown,
    // Scene actions
    ActivateScene,
    // Audio actions
    ToggleMute,
    VolumeDown,
    VolumeUp,
    // Palette completion
    CompleteNext,
    CompletePrev,
}

pub fn handle_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    if model.command_palette.active {
        return match key.code {
            KeyCode::Esc => Some(TuiAction::ClosePalette),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(TuiAction::ClosePalette)
            }
            KeyCode::Enter => Some(TuiAction::PaletteSubmit),
            KeyCode::Backspace => Some(TuiAction::PaletteBackspace),
            KeyCode::Tab => Some(TuiAction::CompleteNext),
            KeyCode::BackTab => Some(TuiAction::CompletePrev),
            KeyCode::Char(c) => Some(TuiAction::PaletteChar(c)),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('q') => Some(TuiAction::Quit),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::Quit)
        }
        KeyCode::Char('/') | KeyCode::Char(':') => Some(TuiAction::OpenPalette),
        KeyCode::Char('r') => Some(TuiAction::ReloadConfig),
        KeyCode::Char('D') => Some(TuiAction::DumpConfig),
        KeyCode::Char('R') => Some(TuiAction::RetryConnect),

        // Panel focus
        KeyCode::Char('s') => Some(TuiAction::FocusScenes),
        KeyCode::Char('a') => Some(TuiAction::FocusAudio),

        // Up/down navigation (arrow keys + vim j/k)
        KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::NavUp),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::NavDown),

        // Activate selected scene (Enter, scenes panel only)
        KeyCode::Enter if model.focus == FocusPanel::Scenes => Some(TuiAction::ActivateScene),

        // Audio-panel actions
        KeyCode::Char('m') if model.focus == FocusPanel::Audio => Some(TuiAction::ToggleMute),
        KeyCode::Left | KeyCode::Char('h') if model.focus == FocusPanel::Audio => {
            Some(TuiAction::VolumeDown)
        }
        KeyCode::Right | KeyCode::Char('l') if model.focus == FocusPanel::Audio => {
            Some(TuiAction::VolumeUp)
        }

        _ => None,
    }
}
