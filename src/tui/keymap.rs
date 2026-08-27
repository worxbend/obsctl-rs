//! Vim / AstroNvim-flavoured key handling.
//!
//! Two things live here: the *pending key* state machine (`g`-prefixed
//! motions and the `<leader>` tree) and the which-key tables that describe
//! each pending menu to the user. Keeping the resolution and the menu in one
//! file is deliberate — a mapping that resolves but isn't listed (or the
//! reverse) is a documentation bug the tests at the bottom catch.

use crate::tui::input::TuiAction;

/// Leader key, matching AstroNvim's default (`<Space>`).
pub const LEADER: char = ' ';

/// A partially-typed key sequence. Anything other than [`Pending::None`]
/// makes the which-key overlay visible and routes the next keypress through
/// [`resolve`] instead of the normal bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pending {
    #[default]
    None,
    /// `g` typed — waiting for the second key of a `g`-motion.
    G,
    /// `<leader>` typed — the which-key root menu is up.
    Leader,
    /// `<leader><group>` typed — a which-key subgroup is up.
    LeaderGroup(char),
}

impl Pending {
    pub fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// One row of a which-key menu. `group` marks a prefix that opens another
/// menu rather than running something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhichKeyEntry {
    pub key: &'static str,
    pub label: &'static str,
    pub group: bool,
}

const fn entry(key: &'static str, label: &'static str) -> WhichKeyEntry {
    WhichKeyEntry {
        key,
        label,
        group: false,
    }
}

const fn group(key: &'static str, label: &'static str) -> WhichKeyEntry {
    WhichKeyEntry {
        key,
        label,
        group: true,
    }
}

const G_MENU: &[WhichKeyEntry] = &[entry("g", "top of list")];

const LEADER_ROOT: &[WhichKeyEntry] = &[
    group("f", "find"),
    group("p", "panel"),
    group("s", "stream"),
    group("c", "config"),
    group("o", "obs"),
    group("u", "ui"),
    // A leaf, not a group: `P` opens the scene-profile editor outright. Note
    // the case — `<leader>p` is the panel group, and a scene profile is not an
    // OBS profile.
    entry("P", "scene profiles"),
    // The cycle key spelled out. `P` on the dashboard does the same thing in
    // one keystroke, but an unlisted key is one nobody finds.
    entry("N", "next scene profile"),
    entry(":", "command palette"),
    entry("q", "quit"),
];

const LEADER_FIND: &[WhichKeyEntry] = &[
    entry("s", "scene"),
    entry("p", "profile"),
    entry("P", "scene profile"),
    entry("c", "collection"),
    entry("a", "audio input"),
];

const LEADER_PANEL: &[WhichKeyEntry] = &[
    entry("s", "scenes"),
    entry("a", "audio"),
    entry("p", "profiles"),
    entry("c", "collections"),
];

const LEADER_STREAM: &[WhichKeyEntry] = &[entry("s", "toggle stream"), entry("r", "toggle record")];

const LEADER_CONFIG: &[WhichKeyEntry] = &[
    entry("r", "reload config"),
    entry("d", "dump config from OBS"),
    entry("v", "validate config"),
];

const LEADER_OBS: &[WhichKeyEntry] = &[
    entry("r", "reconnect to OBS"),
    entry("s", "OBS status"),
    entry("d", "daemon status"),
    entry("c", "reconnect to daemon"),
];

const LEADER_UI: &[WhichKeyEntry] = &[
    entry("t", "theme picker"),
    entry("i", "toggle icons"),
    entry("a", "toggle advanced UI"),
];

/// Title and rows of the which-key overlay for `pending`, or `None` when
/// nothing is pending.
pub fn menu(pending: Pending) -> Option<(&'static str, &'static [WhichKeyEntry])> {
    match pending {
        Pending::None => None,
        Pending::G => Some(("g", G_MENU)),
        Pending::Leader => Some(("<leader>", LEADER_ROOT)),
        Pending::LeaderGroup('f') => Some(("<leader>f  find", LEADER_FIND)),
        Pending::LeaderGroup('p') => Some(("<leader>p  panel", LEADER_PANEL)),
        Pending::LeaderGroup('s') => Some(("<leader>s  stream", LEADER_STREAM)),
        Pending::LeaderGroup('c') => Some(("<leader>c  config", LEADER_CONFIG)),
        Pending::LeaderGroup('o') => Some(("<leader>o  obs", LEADER_OBS)),
        Pending::LeaderGroup('u') => Some(("<leader>u  ui", LEADER_UI)),
        Pending::LeaderGroup(_) => None,
    }
}

/// Resolve the second (or third) key of a pending sequence. `None` means the
/// key isn't mapped here — callers dismiss the menu, which is how which-key
/// behaves for an unknown key.
pub fn resolve(pending: Pending, ch: char) -> Option<TuiAction> {
    match pending {
        Pending::None => None,
        Pending::G => match ch {
            'g' => Some(TuiAction::NavTop),
            _ => None,
        },
        Pending::Leader => match ch {
            'f' | 'p' | 's' | 'c' | 'o' | 'u' => {
                Some(TuiAction::SetPending(Pending::LeaderGroup(ch)))
            }
            ':' => Some(TuiAction::OpenPalette {
                prefix: None,
                seed: "",
            }),
            'q' => Some(TuiAction::Quit),
            'P' => Some(TuiAction::OpenSceneProfiles),
            'N' => Some(TuiAction::SceneProfileCycleNext),
            _ => None,
        },
        Pending::LeaderGroup('f') => match ch {
            's' => Some(seeded("scene ")),
            'p' => Some(seeded("profile ")),
            'P' => Some(seeded("scene-profile ")),
            'c' => Some(seeded("collection ")),
            'a' => Some(seeded("toggle-mute ")),
            _ => None,
        },
        Pending::LeaderGroup('p') => match ch {
            's' => Some(TuiAction::FocusScenes),
            'a' => Some(TuiAction::FocusAudio),
            'p' => Some(TuiAction::FocusProfiles),
            'c' => Some(TuiAction::FocusCollections),
            _ => None,
        },
        Pending::LeaderGroup('s') => match ch {
            's' => Some(TuiAction::ToggleStream),
            'r' => Some(TuiAction::ToggleRecord),
            _ => None,
        },
        Pending::LeaderGroup('c') => match ch {
            'r' => Some(TuiAction::ReloadConfig),
            'd' => Some(TuiAction::DumpConfig),
            'v' => Some(TuiAction::ValidateConfig),
            _ => None,
        },
        Pending::LeaderGroup('o') => match ch {
            'r' => Some(TuiAction::ReconnectObs),
            's' => Some(TuiAction::ObsStatus),
            'd' => Some(TuiAction::ServerStatus),
            'c' => Some(TuiAction::RetryConnect),
            _ => None,
        },
        Pending::LeaderGroup('u') => match ch {
            't' => Some(TuiAction::OpenSettings),
            'i' => Some(TuiAction::ToggleIcons),
            'a' => Some(TuiAction::ToggleAdvancedUi),
            _ => None,
        },
        Pending::LeaderGroup(_) => None,
    }
}

/// Open the palette pre-filled with `seed` after the configured prefix, the
/// way `<leader>f…` mappings jump straight into a filtered command.
fn seeded(seed: &'static str) -> TuiAction {
    TuiAction::OpenPalette { prefix: None, seed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_menus() -> Vec<Pending> {
        vec![
            Pending::G,
            Pending::Leader,
            Pending::LeaderGroup('f'),
            Pending::LeaderGroup('p'),
            Pending::LeaderGroup('s'),
            Pending::LeaderGroup('c'),
            Pending::LeaderGroup('o'),
            Pending::LeaderGroup('u'),
        ]
    }

    #[test]
    fn every_listed_which_key_entry_resolves() {
        for pending in all_menus() {
            let (_, entries) = menu(pending).expect("menu for {pending:?}");
            for e in entries {
                let ch = e.key.chars().next().expect("single-char key");
                let action = resolve(pending, ch)
                    .unwrap_or_else(|| panic!("{pending:?} + '{ch}' is listed but unmapped"));
                if e.group {
                    assert!(
                        matches!(action, TuiAction::SetPending(_)),
                        "{pending:?} + '{ch}' is listed as a group but does not open one"
                    );
                } else {
                    assert!(
                        !matches!(action, TuiAction::SetPending(_)),
                        "{pending:?} + '{ch}' is listed as an action but opens a group"
                    );
                }
            }
        }
    }

    #[test]
    fn every_group_entry_has_a_menu_of_its_own() {
        let (_, root) = menu(Pending::Leader).unwrap();
        for e in root.iter().filter(|e| e.group) {
            let ch = e.key.chars().next().unwrap();
            assert!(
                menu(Pending::LeaderGroup(ch)).is_some(),
                "<leader>{ch} is a group with no which-key menu"
            );
        }
    }

    #[test]
    fn unmapped_keys_do_not_resolve() {
        assert!(resolve(Pending::Leader, 'z').is_none());
        assert!(resolve(Pending::G, 'x').is_none());
        assert!(resolve(Pending::LeaderGroup('f'), 'z').is_none());
        assert!(resolve(Pending::LeaderGroup('z'), 'z').is_none());
        assert!(resolve(Pending::None, 'g').is_none());
    }

    #[test]
    fn pressing_leader_twice_dismisses_rather_than_resolving() {
        assert!(resolve(Pending::Leader, LEADER).is_none());
    }

    #[test]
    fn gg_jumps_to_the_top_of_the_focused_list() {
        assert!(matches!(resolve(Pending::G, 'g'), Some(TuiAction::NavTop)));
    }

    /// `<leader>P` and `<leader>p` are two different mappings, and the case is
    /// the whole difference: `p` opens the panel group, `P` opens the
    /// scene-profile editor. A scene profile is not an OBS profile.
    #[test]
    fn leader_p_is_a_group_and_leader_shift_p_opens_the_scene_profile_editor() {
        assert_eq!(
            resolve(Pending::Leader, 'P'),
            Some(TuiAction::OpenSceneProfiles)
        );
        assert_eq!(
            resolve(Pending::Leader, 'p'),
            Some(TuiAction::SetPending(Pending::LeaderGroup('p')))
        );
    }

    /// The cycle key has to be reachable from the which-key menu as well as
    /// from the dashboard: `<leader>N` is how a user who has never read the
    /// docs finds out that scene profiles can be switched at all.
    #[test]
    fn leader_shift_n_cycles_to_the_next_scene_profile() {
        assert_eq!(
            resolve(Pending::Leader, 'N'),
            Some(TuiAction::SceneProfileCycleNext)
        );
        let (_, root) = menu(Pending::Leader).unwrap();
        assert!(
            root.iter().any(|e| e.key == "N" && !e.group),
            "the cycle key is listed in the which-key root"
        );
    }

    #[test]
    fn leader_find_shift_p_seeds_the_scene_profile_command() {
        assert_eq!(
            resolve(Pending::LeaderGroup('f'), 'P'),
            Some(TuiAction::OpenPalette {
                prefix: None,
                seed: "scene-profile "
            })
        );
    }

    #[test]
    fn leader_find_mappings_seed_the_palette_with_their_command() {
        let action = resolve(Pending::LeaderGroup('f'), 's').unwrap();
        assert!(matches!(
            action,
            TuiAction::OpenPalette {
                prefix: None,
                seed: "scene "
            }
        ));
    }
}
