use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::model::{FocusPanel, TuiModel, View};

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
    FocusProfiles,
    NavUp,
    NavDown,
    // Scene actions
    ActivateScene,
    // Profile actions
    ActivateProfile,
    // Audio actions
    ToggleMute,
    VolumeDown,
    VolumeUp,
    // Palette completion
    CompleteNext,
    CompletePrev,
    // Settings view
    OpenSettings,
    CloseSettings,
    SettingsNavUp,
    SettingsNavDown,
    ApplySettingsTheme,
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

    if model.view == View::Settings {
        return match key.code {
            KeyCode::Esc | KeyCode::F(2) | KeyCode::Char('q') => Some(TuiAction::CloseSettings),
            KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::SettingsNavUp),
            KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::SettingsNavDown),
            KeyCode::Enter => Some(TuiAction::ApplySettingsTheme),
            _ => None,
        };
    }

    match key.code {
        KeyCode::F(2) => Some(TuiAction::OpenSettings),
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiAction::OpenSettings)
        }
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
        KeyCode::Char('p') => Some(TuiAction::FocusProfiles),

        // Up/down navigation (arrow keys + vim j/k)
        KeyCode::Up | KeyCode::Char('k') => Some(TuiAction::NavUp),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiAction::NavDown),

        // Activate selected scene (Enter, scenes panel only)
        KeyCode::Enter if model.focus == FocusPanel::Scenes => Some(TuiAction::ActivateScene),
        // Activate selected profile (Enter, profiles panel only)
        KeyCode::Enter if model.focus == FocusPanel::Profiles => Some(TuiAction::ActivateProfile),

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

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn ctrl_t_opens_settings_from_main_view() {
        let model = TuiModel::default();
        let ctrl_t = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        assert!(matches!(
            handle_key(&model, ctrl_t),
            Some(TuiAction::OpenSettings)
        ));
    }

    #[test]
    fn f2_opens_settings_from_main_view() {
        let model = TuiModel::default();
        assert!(matches!(
            handle_key(&model, key(KeyCode::F(2))),
            Some(TuiAction::OpenSettings)
        ));
    }

    #[test]
    fn settings_view_intercepts_navigation_and_close_keys() {
        let mut model = TuiModel::default();
        model.view = View::Settings;

        assert!(matches!(
            handle_key(&model, key(KeyCode::Down)),
            Some(TuiAction::SettingsNavDown)
        ));
        assert!(matches!(
            handle_key(&model, key(KeyCode::Up)),
            Some(TuiAction::SettingsNavUp)
        ));
        assert!(matches!(
            handle_key(&model, key(KeyCode::Enter)),
            Some(TuiAction::ApplySettingsTheme)
        ));
        assert!(matches!(
            handle_key(&model, key(KeyCode::Esc)),
            Some(TuiAction::CloseSettings)
        ));
        // 'q' closes settings rather than quitting the whole app.
        assert!(matches!(
            handle_key(&model, key(KeyCode::Char('q'))),
            Some(TuiAction::CloseSettings)
        ));

        // Sanity: KeyEventKind default is Press, matching the harness filter.
        assert_eq!(key(KeyCode::Esc).kind, KeyEventKind::Press);
    }
}
