use super::{command::Command, errors::ObsctlError, result::Result};
use crate::support::validation::{
    MAX_TARGET_TOKEN_LENGTH, parse_u8_in_range, trim_and_validate_token_with_max_len,
};

pub fn parse(input: &str) -> Result<Command> {
    let input = input.trim().trim_start_matches('/');
    if input.is_empty() {
        return Err(ObsctlError::CommandParseError("empty command".to_string()));
    }

    let tokens = tokenize(input)?;
    let (cmd_name, args) = tokens
        .split_first()
        .ok_or_else(|| ObsctlError::CommandParseError("empty command".to_string()))?;
    let cmd_key = normalize_command_name(cmd_name)?;
    let maybe_target = args.first();

    match cmd_key.as_str() {
        "help" => expect_args(args, 0, "help", Command::Help),
        "quit" | "exit" => expect_args(args, 0, "quit", Command::Quit),
        "dump-config" => expect_args(args, 0, "dump-config", Command::DumpConfig),
        "reload-config" => expect_args(args, 0, "reload-config", Command::ReloadConfig),
        "status" => expect_args(args, 0, "status", Command::Status),
        "server-status" => expect_args(args, 0, "server-status", Command::ServerStatus),
        "obs-status" => expect_args(args, 0, "obs-status", Command::ObsStatus),
        "validate-config" => expect_args(args, 0, "validate-config", Command::ValidateConfig),
        "reconnect" => expect_args(args, 0, "reconnect", Command::Reconnect),
        "connect" => expect_args(args, 0, "connect", Command::Connect),
        "shutdown-server" => expect_args(args, 0, "shutdown-server", Command::ShutdownServer),
        "stream" => expect_args(args, 0, "stream", Command::ToggleStream),
        "rec" | "record" => expect_args(args, 0, "rec", Command::ToggleRecord),
        "scene" | "set-scene" => {
            if args.len() != 1 {
                return Err(ObsctlError::CommandParseError(format!(
                    "scene expects 1 argument, got {}",
                    args.len()
                )));
            }
            Ok(Command::SetScene {
                target: sanitize_target(maybe_target)?,
            })
        }
        "mute" => {
            if args.len() != 1 {
                return Err(ObsctlError::CommandParseError(format!(
                    "mute expects 1 argument, got {}",
                    args.len()
                )));
            }
            Ok(Command::Mute {
                target: sanitize_target(maybe_target)?,
            })
        }
        "unmute" => {
            if args.len() != 1 {
                return Err(ObsctlError::CommandParseError(format!(
                    "unmute expects 1 argument, got {}",
                    args.len()
                )));
            }
            Ok(Command::Unmute {
                target: sanitize_target(maybe_target)?,
            })
        }
        "toggle-mute" => {
            if args.len() != 1 {
                return Err(ObsctlError::CommandParseError(format!(
                    "toggle-mute expects 1 argument, got {}",
                    args.len()
                )));
            }
            Ok(Command::ToggleMute {
                target: sanitize_target(maybe_target)?,
            })
        }
        "vol" | "volume" => {
            if args.len() != 2 {
                return Err(ObsctlError::CommandParseError(format!(
                    "vol expects 2 arguments, got {}",
                    args.len()
                )));
            }
            if input.trim_end().ends_with('"') {
                return Err(ObsctlError::CommandParseError(
                    "volume percentage must not be quoted".to_string(),
                ));
            }
            let percent = required_u8_percentage(&args[1])?;
            Ok(Command::SetVolume {
                target: sanitize_target(maybe_target)?,
                percent,
            })
        }
        _ => Err(ObsctlError::CommandParseError(format!(
            "unknown command: {cmd_name}"
        ))),
    }
}

fn expect_args(args: &[String], expected: usize, name: &str, cmd: Command) -> Result<Command> {
    if args.len() != expected {
        return Err(ObsctlError::CommandParseError(format!(
            "{name} expects {expected} arguments, got {}",
            args.len()
        )));
    }
    Ok(cmd)
}

fn sanitize_target(value: Option<&String>) -> Result<String> {
    let target = value
        .ok_or_else(|| ObsctlError::CommandParseError("target must not be blank".to_string()))?;
    trim_and_validate_token_with_max_len(target, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::CommandParseError(format!("target {error}")))
}

fn normalize_command_name(value: &str) -> Result<String> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::CommandParseError(format!("command {error}")))
        .map(|value| value.to_ascii_lowercase())
}

fn required_u8_percentage(value: &str) -> Result<u8> {
    parse_u8_in_range(value, "volume", 0, 100).map_err(ObsctlError::CommandParseError)
}

fn tokenize(input: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_quotes = false;

    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                in_quotes = false;
            }
            '"' => {
                in_quotes = true;
            }
            '\\' if in_quotes => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
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
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::command::Command;

    #[test]
    fn parse_simple_commands() {
        assert_eq!(parse("help").unwrap(), Command::Help);
        assert_eq!(parse("/help").unwrap(), Command::Help);
        assert_eq!(parse("quit").unwrap(), Command::Quit);
        assert_eq!(parse("exit").unwrap(), Command::Quit);
        assert_eq!(parse("status").unwrap(), Command::Status);
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
