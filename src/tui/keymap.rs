//! Vim / AstroNvim-flavoured key handling.
//!
//! Two things live here: the *pending key* state machine (`g`-prefixed
//! motions and the `<leader>` tree) and the which-key tables that describe
//! each pending menu to the user. Resolution and menu are the same data: an
//! entry carries the action it runs (or the submenu it opens), so a mapping
//! that resolves but isn't listed — or the reverse — cannot be written.

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

/// What pressing a which-key entry does: open another menu, or run something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// A prefix. `prefix` is the literal key sequence typed so far (drawn
    /// as-is), and `title_key` is the i18n key of the word after it.
    Group {
        prefix: &'static str,
        title_key: &'static str,
        entries: &'static [WhichKeyEntry],
    },
    Action(TuiAction),
}

/// One row of a which-key menu, and the mapping it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhichKeyEntry {
    pub key: &'static str,
    /// i18n key of the row's description; resolved at render time with
    /// `rust_i18n::t!` rather than stored as an English literal.
    pub label_key: &'static str,
    pub outcome: KeyOutcome,
}

impl WhichKeyEntry {
    /// True when pressing this key opens another menu instead of acting.
    pub const fn is_group(&self) -> bool {
        matches!(self.outcome, KeyOutcome::Group { .. })
    }
}

const fn entry(key: &'static str, label_key: &'static str, action: TuiAction) -> WhichKeyEntry {
    WhichKeyEntry {
        key,
        label_key,
        outcome: KeyOutcome::Action(action),
    }
}

const fn group(
    key: &'static str,
    label_key: &'static str,
    prefix: &'static str,
    entries: &'static [WhichKeyEntry],
) -> WhichKeyEntry {
    WhichKeyEntry {
        key,
        label_key,
        outcome: KeyOutcome::Group {
            prefix,
            title_key: label_key,
            entries,
        },
    }
}

/// Open the palette pre-filled with `seed` after the configured prefix, the
/// way `<leader>f…` mappings jump straight into a filtered command.
const fn seeded(seed: &'static str) -> TuiAction {
    TuiAction::OpenPalette { prefix: None, seed }
}

const G_MENU: &[WhichKeyEntry] = &[entry("g", "tui.whichkey.g.top_of_list", TuiAction::NavTop)];

const LEADER_ROOT: &[WhichKeyEntry] = &[
    group("f", "tui.whichkey.leader.find", "<leader>f", LEADER_FIND),
    group("p", "tui.whichkey.leader.panel", "<leader>p", LEADER_PANEL),
    group(
        "s",
        "tui.whichkey.leader.stream",
        "<leader>s",
        LEADER_STREAM,
    ),
    group(
        "c",
        "tui.whichkey.leader.config",
        "<leader>c",
        LEADER_CONFIG,
    ),
    group("o", "tui.whichkey.leader.obs", "<leader>o", LEADER_OBS),
    group("u", "tui.whichkey.leader.ui", "<leader>u", LEADER_UI),
    // A leaf, not a group: `P` opens the scene-profile editor outright. Note
    // the case — `<leader>p` is the panel group, and a scene profile is not an
    // OBS profile.
    entry(
        "P",
        "tui.whichkey.leader.scene_profiles",
        TuiAction::OpenSceneProfiles,
    ),
    // The cycle key spelled out. `P` on the dashboard does the same thing in
    // one keystroke, but an unlisted key is one nobody finds.
    entry(
        "N",
        "tui.whichkey.leader.next_scene_profile",
        TuiAction::SceneProfileCycleNext,
    ),
    entry(
        ":",
        "tui.whichkey.leader.command_palette",
        TuiAction::OpenPalette {
            prefix: None,
            seed: "",
        },
    ),
    entry("q", "tui.whichkey.leader.quit", TuiAction::Quit),
];

const LEADER_FIND: &[WhichKeyEntry] = &[
    entry("s", "tui.whichkey.find.scene", seeded("scene ")),
    entry("p", "tui.whichkey.find.profile", seeded("profile ")),
    entry(
        "P",
        "tui.whichkey.find.scene_profile",
        seeded("scene-profile "),
    ),
    entry("c", "tui.whichkey.find.collection", seeded("collection ")),
    entry("a", "tui.whichkey.find.audio_input", seeded("toggle-mute ")),
];

const LEADER_PANEL: &[WhichKeyEntry] = &[
    entry("s", "tui.whichkey.panel.scenes", TuiAction::FocusScenes),
    entry("a", "tui.whichkey.panel.audio", TuiAction::FocusAudio),
    entry("p", "tui.whichkey.panel.profiles", TuiAction::FocusProfiles),
    entry(
        "c",
        "tui.whichkey.panel.collections",
        TuiAction::FocusCollections,
    ),
];

const LEADER_STREAM: &[WhichKeyEntry] = &[
    entry(
        "s",
        "tui.whichkey.stream.toggle_stream",
        TuiAction::ToggleStream,
    ),
    entry(
        "r",
        "tui.whichkey.stream.toggle_record",
        TuiAction::ToggleRecord,
    ),
];

const LEADER_CONFIG: &[WhichKeyEntry] = &[
    entry("r", "tui.whichkey.config.reload", TuiAction::ReloadConfig),
    entry("d", "tui.whichkey.config.dump", TuiAction::DumpConfig),
    entry(
        "v",
        "tui.whichkey.config.validate",
        TuiAction::ValidateConfig,
    ),
];

const LEADER_OBS: &[WhichKeyEntry] = &[
    entry(
        "r",
        "tui.whichkey.obs.reconnect_obs",
        TuiAction::ReconnectObs,
    ),
    entry("s", "tui.whichkey.obs.obs_status", TuiAction::ObsStatus),
    entry(
        "d",
        "tui.whichkey.obs.daemon_status",
        TuiAction::ServerStatus,
    ),
    entry(
        "c",
        "tui.whichkey.obs.reconnect_daemon",
        TuiAction::RetryConnect,
    ),
];

const LEADER_UI: &[WhichKeyEntry] = &[
    entry("t", "tui.whichkey.ui.theme_picker", TuiAction::OpenSettings),
    entry("i", "tui.whichkey.ui.toggle_icons", TuiAction::ToggleIcons),
    entry(
        "a",
        "tui.whichkey.ui.toggle_advanced",
        TuiAction::ToggleAdvancedUi,
    ),
];

/// The two halves of a which-key menu heading: the literal key sequence
/// typed so far, and (for a subgroup) the i18n key of the word that names it.
/// Kept apart so the key sequence stays verbatim while the word is localized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuTitle {
    pub prefix: &'static str,
    pub title_key: Option<&'static str>,
}

impl MenuTitle {
    const fn bare(prefix: &'static str) -> Self {
        Self {
            prefix,
            title_key: None,
        }
    }
}

/// Title and rows of the which-key overlay for `pending`, or `None` when
/// nothing is pending.
pub fn menu(pending: Pending) -> Option<(MenuTitle, &'static [WhichKeyEntry])> {
    match pending {
        Pending::None => None,
        Pending::G => Some((MenuTitle::bare("g"), G_MENU)),
        Pending::Leader => Some((MenuTitle::bare("<leader>"), LEADER_ROOT)),
        // Only a root entry that is actually a group opens a submenu: keys
        // like `<leader>P` start with a letter but are leaves.
        Pending::LeaderGroup(ch) => {
            LEADER_ROOT
                .iter()
                .find(|e| e.key.starts_with(ch))
                .and_then(|e| match &e.outcome {
                    KeyOutcome::Group {
                        prefix,
                        title_key,
                        entries,
                    } => Some((
                        MenuTitle {
                            prefix,
                            title_key: Some(title_key),
                        },
                        *entries,
                    )),
                    KeyOutcome::Action(_) => None,
                })
        }
    }
}

/// Resolve the second (or third) key of a pending sequence. `None` means the
/// key isn't mapped here — callers dismiss the menu, which is how which-key
/// behaves for an unknown key.
pub fn resolve(pending: Pending, ch: char) -> Option<TuiAction> {
    let (_, entries) = menu(pending)?;
    entries
        .iter()
        .find(|e| e.key.starts_with(ch))
        .map(|e| match &e.outcome {
            KeyOutcome::Group { .. } => TuiAction::SetPending(Pending::LeaderGroup(ch)),
            KeyOutcome::Action(action) => action.clone(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_tables() -> Vec<(&'static str, &'static [WhichKeyEntry])> {
        vec![
            ("g", G_MENU),
            ("<leader>", LEADER_ROOT),
            ("<leader>f", LEADER_FIND),
            ("<leader>p", LEADER_PANEL),
            ("<leader>s", LEADER_STREAM),
            ("<leader>c", LEADER_CONFIG),
            ("<leader>o", LEADER_OBS),
            ("<leader>u", LEADER_UI),
        ]
    }

    /// Lookup takes the *first* entry whose key matches, so a duplicated key
    /// in a table would silently shadow the later one — with no compile error
    /// and no visible sign except a mapping that never fires.
    #[test]
    fn no_table_lists_the_same_key_twice() {
        for (name, entries) in all_tables() {
            let mut seen: Vec<&str> = Vec::new();
            for e in entries {
                assert!(
                    !seen.contains(&e.key),
                    "{name} lists the key '{}' twice; the second one is unreachable",
                    e.key
                );
                seen.push(e.key);
            }
        }
    }

    #[test]
    fn every_group_entry_has_a_menu_of_its_own() {
        for e in LEADER_ROOT.iter().filter(|e| e.is_group()) {
            let ch = e.key.chars().next().unwrap();
            let (_, entries) = menu(Pending::LeaderGroup(ch))
                .unwrap_or_else(|| panic!("<leader>{ch} is a group with no which-key menu"));
            assert!(
                !entries.is_empty(),
                "<leader>{ch} opens an empty which-key menu"
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
        // A leaf key never opens a menu, even though it looks like a prefix.
        assert!(menu(Pending::LeaderGroup('P')).is_none());
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
            root.iter().any(|e| e.key == "N" && !e.is_group()),
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
