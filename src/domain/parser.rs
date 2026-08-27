use super::{
    command::Command,
    errors::ObsctlError,
    names::{checked_name, normalized_name},
    result::Result,
};

/// Command-line prefixes the TUI palette may open with. Both are stripped
/// before parsing so `:scene Main`, `/scene Main`, and `scene Main` are the
/// same command (see `ui.command_palette_prefix`).
pub const PALETTE_PREFIXES: [char; 2] = ['/', ':'];

/// Prefix the TUI command line opens with unless `ui.command_palette_prefix`
/// says otherwise. `:` mirrors vim's command prompt.
pub const DEFAULT_PALETTE_PREFIX: char = ':';

/// One word of the palette's vocabulary: what it is called, what else it
/// answers to, how many arguments it takes, and what it builds.
///
/// The alternative — and what this replaced — was a `match` on the command
/// name with the alternative spellings written into the arm patterns and the
/// argument count repeated in every arm. Because the alias list only existed
/// inside those patterns, nothing could read it: the completion menu and the
/// `:help` text each kept their own copy of the vocabulary, and all three had
/// drifted apart. Here the vocabulary is data, so the list the palette offers
/// is derived from it rather than maintained beside it.
///
/// This mirrors the `CommandSpec` table `crate::ipc::protocol` uses for the
/// IPC command names, deliberately: the two are the same kind of thing seen
/// from two sides.
struct PaletteCommandSpec {
    /// The spelling shown in menus and used in error messages.
    canonical: &'static str,
    /// Other spellings accepted for the same command. Not offered by
    /// completion — see [`CANONICAL_PALETTE_COMMANDS`].
    aliases: &'static [&'static str],
    /// How many arguments follow the command name. Checked once, in [`parse`],
    /// before `build` runs, so `build` can index its arguments directly.
    arity: usize,
    /// Turns the already-counted arguments into a [`Command`].
    build: fn(&[Token]) -> Result<Command>,
}

/// Every command the palette understands, in the order the completion menu
/// offers them.
const PALETTE_COMMANDS: &[PaletteCommandSpec] = &[
    PaletteCommandSpec {
        canonical: "help",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::Help),
    },
    PaletteCommandSpec {
        canonical: "themes",
        aliases: &["theme", "settings"],
        arity: 0,
        build: |_| Ok(Command::Themes),
    },
    PaletteCommandSpec {
        canonical: "scene",
        aliases: &["set-scene"],
        arity: 1,
        build: |args| {
            Ok(Command::SetScene {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "profile",
        aliases: &["set-profile"],
        arity: 1,
        build: |args| {
            Ok(Command::SetProfile {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "collection",
        aliases: &["set-collection", "scene-collection"],
        arity: 1,
        build: |args| {
            Ok(Command::SetSceneCollection {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "scene-profile",
        aliases: &["set-scene-profile"],
        arity: 1,
        build: |args| {
            Ok(Command::SetSceneProfile {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "scene-profile-off",
        aliases: &["scene-profile-clear"],
        arity: 0,
        build: |_| Ok(Command::ClearSceneProfile),
    },
    // Removing a scene profile is its own word rather than a flag on
    // `scene-profile`, because the palette has no flags: every command here is
    // a name followed by a fixed number of arguments.
    PaletteCommandSpec {
        canonical: "scene-profile-delete",
        aliases: &["delete-scene-profile"],
        arity: 1,
        build: |args| {
            Ok(Command::DeleteSceneProfile {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "mute",
        aliases: &[],
        arity: 1,
        build: |args| {
            Ok(Command::Mute {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "unmute",
        aliases: &[],
        arity: 1,
        build: |args| {
            Ok(Command::Unmute {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "toggle-mute",
        aliases: &[],
        arity: 1,
        build: |args| {
            Ok(Command::ToggleMute {
                target: target_of(args)?,
            })
        },
    },
    PaletteCommandSpec {
        canonical: "vol",
        aliases: &["volume"],
        arity: 2,
        build: build_set_volume,
    },
    PaletteCommandSpec {
        canonical: "stream",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::ToggleStream),
    },
    PaletteCommandSpec {
        canonical: "rec",
        aliases: &["record"],
        arity: 0,
        build: |_| Ok(Command::ToggleRecord),
    },
    PaletteCommandSpec {
        canonical: "status",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::Status),
    },
    PaletteCommandSpec {
        canonical: "obs-status",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::ObsStatus),
    },
    PaletteCommandSpec {
        canonical: "server-status",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::ServerStatus),
    },
    PaletteCommandSpec {
        canonical: "reload-config",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::ReloadConfig),
    },
    PaletteCommandSpec {
        canonical: "dump-config",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::DumpConfig),
    },
    PaletteCommandSpec {
        canonical: "validate-config",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::ValidateConfig),
    },
    PaletteCommandSpec {
        canonical: "reconnect",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::Reconnect),
    },
    PaletteCommandSpec {
        canonical: "connect",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::Connect),
    },
    PaletteCommandSpec {
        canonical: "shutdown-server",
        aliases: &[],
        arity: 0,
        build: |_| Ok(Command::ShutdownServer),
    },
    PaletteCommandSpec {
        canonical: "quit",
        aliases: &["exit"],
        arity: 0,
        build: |_| Ok(Command::Quit),
    },
];

const PALETTE_COMMAND_COUNT: usize = PALETTE_COMMANDS.len();

/// Copies the canonical spelling out of every row of [`PALETTE_COMMANDS`].
///
/// Written as a `const fn` with an index loop because iterators are not
/// available in a constant, and the list has to be a constant: it is published
/// as [`CANONICAL_PALETTE_COMMANDS`] and read by the TUI at startup.
const fn canonical_names() -> [&'static str; PALETTE_COMMAND_COUNT] {
    let mut names = [""; PALETTE_COMMAND_COUNT];
    let mut index = 0;
    while index < PALETTE_COMMAND_COUNT {
        names[index] = PALETTE_COMMANDS[index].canonical;
        index += 1;
    }
    names
}

const CANONICAL_NAMES: [&str; PALETTE_COMMAND_COUNT] = canonical_names();

/// Every command the palette offers, in the canonical spelling.
///
/// Derived from [`PALETTE_COMMANDS`] rather than written out, because three
/// copies of this vocabulary had already drifted apart: the completion list,
/// the `:help` text, and the parser itself. `connect` and `shutdown-server`
/// are real commands that neither of the other two mentioned, so users had no
/// way to discover them.
///
/// Aliases (`set-scene`, `theme`, `settings`, `volume`, `record`, `exit`) are
/// deliberately absent. They stay accepted by [`parse`]; listing them as well
/// would double the length of every completion menu without offering anything
/// new.
pub const CANONICAL_PALETTE_COMMANDS: &[&str] = &CANONICAL_NAMES;

/// One argument as the tokenizer produced it.
///
/// `quoted` is kept because one rule depends on it: a volume percentage has to
/// be a bare number, so `vol mic "70"` is a mistake worth reporting rather
/// than a target named `70`. The tokenizer used to return plain strings and
/// that rule was then guessed at from the raw input line — by testing whether
/// the line ended in a quote — which both missed `vol mic "7"0` (quoted, but
/// not ending in a quote) and would have misfired on any future command whose
/// last argument was legitimately quoted.
struct Token {
    text: String,
    quoted: bool,
}

pub fn parse(input: &str) -> Result<Command> {
    let input = input.trim().trim_start_matches(PALETTE_PREFIXES);
    if input.is_empty() {
        return Err(ObsctlError::CommandParseError("empty command".to_string()));
    }

    let tokens = tokenize(input)?;
    let (head, args) = tokens
        .split_first()
        .ok_or_else(|| ObsctlError::CommandParseError("empty command".to_string()))?;

    let name = normalize_command_name(&head.text)?;
    let spec = PALETTE_COMMANDS
        .iter()
        .find(|spec| spec.canonical == name || spec.aliases.contains(&name.as_str()))
        .ok_or_else(|| ObsctlError::CommandParseError(format!("unknown command: {}", head.text)))?;

    if args.len() != spec.arity {
        // "1 argument", but "0 arguments" and "2 arguments".
        let noun = if spec.arity == 1 {
            "argument"
        } else {
            "arguments"
        };
        return Err(ObsctlError::CommandParseError(format!(
            "{} expects {} {noun}, got {}",
            spec.canonical,
            spec.arity,
            args.len()
        )));
    }

    (spec.build)(args)
}

/// The single target argument of a one-argument command. Safe to index
/// because [`parse`] has already checked the count.
fn target_of(args: &[Token]) -> Result<String> {
    sanitize_target(&args[0].text)
}

/// `vol <target> <percent>` is the only command with two arguments and the
/// only one that rejects a quoted argument, so it gets a named builder rather
/// than an inline one.
fn build_set_volume(args: &[Token]) -> Result<Command> {
    if args[1].quoted {
        return Err(ObsctlError::CommandParseError(
            "volume percentage must not be quoted".to_string(),
        ));
    }
    let percent = parse_volume_percent(&args[1].text)?;
    Ok(Command::SetVolume {
        target: sanitize_target(&args[0].text)?,
        percent,
    })
}

fn sanitize_target(value: &str) -> Result<String> {
    checked_name(value).map_err(|error| ObsctlError::CommandParseError(format!("target {error}")))
}

fn normalize_command_name(value: &str) -> Result<String> {
    normalized_name(value)
        .map_err(|error| ObsctlError::CommandParseError(format!("command {error}")))
}

/// Parse `value` as a volume percentage, an integer between 0 and 100,
/// describing any failure in the words the user would recognise, e.g.
/// `volume must be an integer 0-100, got "loud"`.
///
/// Surrounding whitespace is a failure rather than something to trim: a
/// palette argument reaches here already tokenized, so a space inside it means
/// the user quoted something they did not mean to.
fn parse_volume_percent(value: &str) -> Result<u8> {
    if value.trim() != value {
        return Err(ObsctlError::CommandParseError(format!(
            "volume must be an integer 0-100, got {value:?}"
        )));
    }

    let parsed = value.parse::<u64>().map_err(|_| {
        ObsctlError::CommandParseError(format!("volume must be an integer 0-100, got {value:?}"))
    })?;

    if parsed > 100 {
        return Err(ObsctlError::CommandParseError(format!(
            "volume must be 0-100, got {parsed}"
        )));
    }

    Ok(parsed as u8)
}

/// Split a command line into arguments, honouring double quotes and
/// backslash escapes inside them.
///
/// Each token records whether any quote character took part in producing it,
/// which is the one thing about the original spelling a later rule needs; see
/// [`Token`].
fn tokenize(input: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_quoted = false;
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;

    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                in_quotes = false;
                current_quoted = true;
            }
            '"' => {
                in_quotes = true;
                current_quoted = true;
            }
            '\\' if in_quotes => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(Token {
                        text: std::mem::take(&mut current),
                        quoted: std::mem::take(&mut current_quoted),
                    });
                }
            }
            _ => current.push(c),
        }
    }

    if in_quotes {
        return Err(ObsctlError::CommandParseError(
            "unterminated quote".to_string(),
        ));
    }

    if !current.is_empty() {
        tokens.push(Token {
            text: current,
            quoted: current_quoted,
        });
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {

    /// A command line that satisfies `spec`: the name followed by as many
    /// plausible arguments as its arity calls for. Derived from the table
    /// rather than written per command, so a new row needs no new case here.
    fn example_input(name: &str, arity: usize) -> String {
        match arity {
            0 => name.to_string(),
            1 => format!("{name} Main"),
            // Only `vol` takes two, and its second argument is a percentage.
            _ => format!("{name} Mic 50"),
        }
    }

    /// Every name the palette offers must be one `parse` accepts. The two used
    /// to be separate lists that had already diverged; the list is now derived
    /// from the parser's own table, and this pins that the derivation holds.
    #[test]
    fn every_canonical_command_parses() {
        for spec in PALETTE_COMMANDS {
            let input = example_input(spec.canonical, spec.arity);
            assert!(
                parse(&input).is_ok(),
                "`{input}` is offered by the palette but rejected by the parser"
            );
        }
    }

    /// The published list is exactly the table's canonical names, in the
    /// table's order. The TUI reads it for completion, so a reordered or
    /// renamed row is a user-visible change.
    #[test]
    fn canonical_commands_match_the_table() {
        let from_table: Vec<&str> = PALETTE_COMMANDS.iter().map(|spec| spec.canonical).collect();
        assert_eq!(CANONICAL_PALETTE_COMMANDS, from_table.as_slice());
    }

    /// An alias is another spelling of the same command, never a different
    /// one. When aliases lived inside `match` patterns nothing could check
    /// that; now they are data and this walks all of them.
    #[test]
    fn every_alias_parses_to_its_canonical_command() {
        for spec in PALETTE_COMMANDS {
            let canonical = parse(&example_input(spec.canonical, spec.arity)).unwrap();
            for alias in spec.aliases {
                let aliased = parse(&example_input(alias, spec.arity)).unwrap();
                assert_eq!(
                    aliased, canonical,
                    "`{alias}` should mean the same as `{}`",
                    spec.canonical
                );
            }
        }
    }

    /// No duplicates, so a completion menu cannot list the same command twice.
    #[test]
    fn canonical_commands_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for name in CANONICAL_PALETTE_COMMANDS {
            assert!(seen.insert(*name), "duplicate palette command: {name}");
        }
    }
    use super::*;
    use crate::domain::command::Command;
    use crate::support::validation::MAX_TARGET_TOKEN_LENGTH;

    #[test]
    fn parse_volume_percent_validates_integer_bounds() {
        assert_eq!(parse_volume_percent("42").unwrap(), 42);
        assert_eq!(parse_volume_percent("0").unwrap(), 0);
        assert_eq!(parse_volume_percent("100").unwrap(), 100);
        assert!(parse_volume_percent("101").is_err());
        assert!(parse_volume_percent("-1").is_err());
        assert!(parse_volume_percent("50.5").is_err());
        assert!(parse_volume_percent(" 42").is_err());
        assert!(parse_volume_percent("42 ").is_err());
    }

    /// A scene profile is a set of scene-visibility choices, and `profile` is
    /// the OBS profile: two commands, never one.
    #[test]
    fn scene_profile_commands_are_separate_from_the_obs_profile_command() {
        assert_eq!(
            parse(":scene-profile streaming").unwrap(),
            Command::SetSceneProfile {
                target: "streaming".to_string()
            }
        );
        assert_eq!(
            parse(":set-scene-profile streaming").unwrap(),
            Command::SetSceneProfile {
                target: "streaming".to_string()
            }
        );
        assert_eq!(
            parse(":scene-profile-off").unwrap(),
            Command::ClearSceneProfile
        );
        assert_eq!(
            parse(":scene-profile-clear").unwrap(),
            Command::ClearSceneProfile
        );
        assert_eq!(
            parse(":profile streaming").unwrap(),
            Command::SetProfile {
                target: "streaming".to_string()
            }
        );
        // Switching profiles is not something to do by accident, so neither
        // command guesses at a missing or extra argument.
        assert!(parse(":scene-profile").is_err());
        assert!(parse(":scene-profile-off now").is_err());
    }

    /// Deleting is the third scene-profile verb, and it is deliberately not
    /// reachable by leaving the name off `scene-profile-off`: it needs the name
    /// of the profile to remove, and nothing else can stand in for it.
    #[test]
    fn a_scene_profile_can_be_deleted_by_name_from_the_palette() {
        assert_eq!(
            parse(":scene-profile-delete night").unwrap(),
            Command::DeleteSceneProfile {
                target: "night".to_string()
            }
        );
        assert_eq!(
            parse(":delete-scene-profile night").unwrap(),
            Command::DeleteSceneProfile {
                target: "night".to_string()
            }
        );
        assert!(parse(":scene-profile-delete").is_err());
        assert!(parse(":scene-profile-delete one two").is_err());
    }

    #[test]
    fn parse_accepts_either_palette_prefix() {
        assert_eq!(parse(":help").unwrap(), Command::Help);
        assert_eq!(parse("/help").unwrap(), Command::Help);
        assert_eq!(
            parse(":scene Main").unwrap(),
            Command::SetScene {
                target: "Main".to_string()
            }
        );
        assert!(parse(":").is_err());
    }

    #[test]
    fn parse_simple_commands() {
        assert_eq!(parse("help").unwrap(), Command::Help);
        assert_eq!(parse("/help").unwrap(), Command::Help);
        assert_eq!(parse("quit").unwrap(), Command::Quit);
        assert_eq!(parse("exit").unwrap(), Command::Quit);
        assert_eq!(parse("status").unwrap(), Command::Status);
        assert_eq!(parse("themes").unwrap(), Command::Themes);
        assert_eq!(parse("theme").unwrap(), Command::Themes);
        assert_eq!(parse("settings").unwrap(), Command::Themes);
        assert!(parse("themes extra").is_err());
    }

    #[test]
    fn parse_scene_command() {
        assert_eq!(
            parse("scene main").unwrap(),
            Command::SetScene {
                target: "main".to_string()
            }
        );
        assert_eq!(
            parse(r#"scene "Main Camera""#).unwrap(),
            Command::SetScene {
                target: "Main Camera".to_string()
            }
        );
    }

    #[test]
    fn parse_profile_command() {
        assert_eq!(
            parse("profile Streaming").unwrap(),
            Command::SetProfile {
                target: "Streaming".to_string()
            }
        );
        assert!(parse("profile").is_err());
        assert!(parse("profile a b").is_err());
    }

    #[test]
    fn parse_scene_collection_command() {
        assert_eq!(
            parse("collection Podcast").unwrap(),
            Command::SetSceneCollection {
                target: "Podcast".to_string()
            }
        );
        assert_eq!(
            parse("scene-collection Podcast").unwrap(),
            Command::SetSceneCollection {
                target: "Podcast".to_string()
            }
        );
        assert!(parse("collection").is_err());
        assert!(parse("collection a b").is_err());
    }

    #[test]
    fn parse_rejects_blank_target() {
        assert!(parse("scene").is_err());
        assert!(parse("scene   ").is_err());
        assert!(parse(r#"scene "   ""#).is_err());
    }

    #[test]
    fn parse_rejects_control_characters_in_target() {
        assert!(parse("scene main\tcam").is_err());
    }

    #[test]
    fn parse_rejects_control_characters_in_command_name() {
        assert!(parse("main\0cam").is_err());
    }

    #[test]
    fn parse_rejects_oversize_command_name() {
        let input = format!("{} arg", "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1));
        assert!(parse(&input).is_err());
    }

    #[test]
    fn parse_rejects_oversize_target() {
        let input = format!("scene {}", "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1));
        assert!(parse(&input).is_err());
    }

    #[test]
    fn parse_volume_command() {
        assert_eq!(
            parse("vol mic 70").unwrap(),
            Command::SetVolume {
                target: "mic".to_string(),
                percent: 70
            }
        );
        assert!(parse("vol mic 101").is_err());
        assert!(parse("vol mic abc").is_err());
    }

    #[test]
    fn parse_unterminated_quote() {
        assert!(parse(r#"scene "Main"#).is_err());
    }

    #[test]
    fn parse_unknown_command() {
        assert!(parse("frobnicate").is_err());
    }

    #[test]
    fn parse_empty_command() {
        assert!(parse("").is_err());
        assert!(parse("/").is_err());
    }

    #[test]
    fn parse_escaped_quote_in_name() {
        // A quoted argument with an escaped inner quote
        let result = parse(r#"scene "Main \"Live\" Cam""#).unwrap();
        assert_eq!(
            result,
            Command::SetScene {
                target: r#"Main "Live" Cam"#.to_string()
            }
        );
    }

    #[test]
    fn parse_set_scene_alias() {
        // set-scene is an alias for scene
        assert_eq!(
            parse("set-scene main").unwrap(),
            Command::SetScene {
                target: "main".to_string()
            }
        );
    }

    #[test]
    fn parse_volume_alias() {
        // volume is an alias for vol
        assert_eq!(
            parse("volume mic 50").unwrap(),
            Command::SetVolume {
                target: "mic".to_string(),
                percent: 50
            }
        );
    }

    #[test]
    fn parse_volume_command_rejects_invalid_values() {
        assert!(parse("vol mic 101").is_err());
        assert!(parse("vol mic 50.5").is_err());
        assert!(parse(r#"vol mic "70""#).is_err());
    }

    /// Both of these spell a quoted percentage; they differ only in where the
    /// closing quote sits. The rule used to be guessed at from the raw input
    /// line ending in a quote, so the second one was accepted as `70`.
    #[test]
    fn parse_volume_rejects_a_quoted_percentage_wherever_the_quotes_are() {
        assert!(parse(r#"vol mic "70""#).is_err());
        assert!(parse(r#"vol mic "7"0"#).is_err());
        assert!(parse(r#"vol mic 7"0""#).is_err());
    }

    /// Only the percentage has to be unquoted. A target with spaces in it can
    /// only be written with quotes, so quoting one must stay allowed.
    #[test]
    fn parse_volume_accepts_a_quoted_target() {
        assert_eq!(
            parse(r#"vol "Main Mic" 40"#).unwrap(),
            Command::SetVolume {
                target: "Main Mic".to_string(),
                percent: 40
            }
        );
    }

    #[test]
    fn parse_command_names_case_insensitively() {
        assert_eq!(
            parse("/SCENE Main").unwrap(),
            Command::SetScene {
                target: "Main".to_string()
            }
        );
        assert_eq!(
            parse("MUTE Mic").unwrap(),
            Command::Mute {
                target: "Mic".to_string()
            }
        );
        assert_eq!(
            parse("VoL Mic 50").unwrap(),
            Command::SetVolume {
                target: "Mic".to_string(),
                percent: 50
            }
        );
        assert_eq!(parse("/REC").unwrap(), Command::ToggleRecord);
    }

    #[test]
    fn parse_volume_boundaries() {
        assert!(parse("vol mic 0").is_ok());
        assert!(parse("vol mic 100").is_ok());
        assert!(parse("vol mic 101").is_err());
    }

    #[test]
    fn parse_wrong_arg_count_fails() {
        assert!(parse("scene").is_err());
        assert!(parse("scene a b").is_err());
        assert!(parse("mute").is_err());
        assert!(parse("vol mic").is_err()); // missing percent
        assert!(parse("help extra_arg").is_err());
    }

    #[test]
    fn parse_all_zero_arg_commands() {
        for cmd in &[
            "dump-config",
            "reload-config",
            "server-status",
            "obs-status",
            "validate-config",
            "reconnect",
            "connect",
            "shutdown-server",
        ] {
            assert!(parse(cmd).is_ok(), "command '{cmd}' should parse ok");
        }
    }
}
