use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::model::TuiModel;

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
}

pub fn handle_key(model: &TuiModel, key: KeyEvent) -> Option<TuiAction> {
    if model.command_palette.active {
        match key.code {
            KeyCode::Esc => Some(TuiAction::ClosePalette),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(TuiAction::ClosePalette)
            }
            KeyCode::Enter => Some(TuiAction::PaletteSubmit),
            KeyCode::Backspace => Some(TuiAction::PaletteBackspace),
            KeyCode::Char(c) => Some(TuiAction::PaletteChar(c)),
            _ => None,
        }
    } else {
        match key.code {
            KeyCode::Char('q') => Some(TuiAction::Quit),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(TuiAction::Quit)
            }
            KeyCode::Char('/') | KeyCode::Char(':') => Some(TuiAction::OpenPalette),
            KeyCode::Char('r') => Some(TuiAction::ReloadConfig),
            KeyCode::Char('D') => Some(TuiAction::DumpConfig),
            KeyCode::Char('R') => Some(TuiAction::RetryConnect),
            _ => None,
        }
    }
}
