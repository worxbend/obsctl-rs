//! Everything the TUI sends to the daemon, and how it renders what comes
//! back.
//!
//! The TUI is an IPC client like the CLI: it never opens a WebSocket to OBS
//! itself. Keeping the payload building, the target validation, and the
//! one-line reply rendering together means the rules about what may be put on
//! the wire are read in one place rather than beside the key bindings that
//! happen to use them.

use std::path::Path;

use crate::{
    domain::{command::Command, names::checked_name, parser, result::Result},
    ipc::protocol::{CommandPayload, ServerCommand, ServerMessage},
    tui::session::send_command,
};

/// The `:help` one-liner, built from the same list the completion menu uses so
/// the two cannot come to disagree about what exists.
fn help_line() -> String {
    let commands: Vec<String> = parser::CANONICAL_PALETTE_COMMANDS
        .iter()
        .map(|name| format!(":{name}"))
        .collect();
    format!(
        "Commands: {}  —  <space> opens the which-key menu",
        commands.join(" ")
    )
}

/// What the event loop should do about a command typed into the palette.
///
/// Two of these used to be signalled by returning the literal strings "quit"
/// and "themes" for the caller to compare against — in the same return value
/// that otherwise carries the daemon's own reply text. A daemon message of
/// exactly "quit" therefore ended the user's session. Distinct variants make
/// the instruction and the text impossible to confuse.
pub(super) enum PaletteOutcome {
    /// Leave the TUI.
    Quit,
    /// Switch to the settings view.
    OpenSettings,
    /// Show this on the status line.
    Status(String),
}

pub(super) async fn dispatch_palette_command(socket_path: &Path, input: &str) -> PaletteOutcome {
    let command = match parser::parse(input) {
        Ok(command) => command,
        Err(e) => return PaletteOutcome::Status(format!("error: {e}")),
    };

    match command {
        Command::Quit => PaletteOutcome::Quit,
        Command::Themes => PaletteOutcome::OpenSettings,
        Command::Help => PaletteOutcome::Status(help_line()),
        command => {
            let payload = match command_to_payload(command) {
                Ok(payload) => payload,
                Err(error) => return PaletteOutcome::Status(format!("error: {error}")),
            };
            PaletteOutcome::Status(
                ReplyStyle::Acknowledge.format(send_command(socket_path, payload).await, "ok"),
            )
        }
    }
}

/// Turn a parsed palette command into the IPC payload that carries it.
///
/// Built through `CommandPayload`'s constructors rather than by writing the
/// `{"target": ...}` envelopes out by hand — seven of them, in a file that
/// already used the constructors correctly two functions further down, and
/// beside a CLI that uses them throughout. Hand-written wire payloads are how
/// a key gets misspelled in one command and not the others.
fn command_to_payload(cmd: Command) -> std::result::Result<CommandPayload, String> {
    let with_target = |command, target: String| {
        sanitize_target_arg(&target).map(|target| CommandPayload::with_target(command, &target))
    };

    match cmd {
        Command::Status => Ok(CommandPayload::simple(ServerCommand::GetSnapshot)),
        Command::ServerStatus => Ok(CommandPayload::simple(ServerCommand::GetServerStatus)),
        Command::ObsStatus => Ok(CommandPayload::simple(ServerCommand::GetObsStatus)),
        Command::ValidateConfig => Ok(CommandPayload::simple(ServerCommand::ValidateConfig)),
        Command::Reconnect | Command::Connect => {
            Ok(CommandPayload::simple(ServerCommand::ReconnectObs))
        }
        Command::ShutdownServer => Ok(CommandPayload::simple(ServerCommand::ShutdownServer)),
        Command::DumpConfig => Ok(CommandPayload::simple(ServerCommand::DumpConfig)),
        Command::ReloadConfig => Ok(CommandPayload::simple(ServerCommand::ReloadConfig)),
        Command::ToggleStream => Ok(CommandPayload::simple(ServerCommand::ToggleStream)),
        Command::ToggleRecord => Ok(CommandPayload::simple(ServerCommand::ToggleRecord)),

        Command::SetScene { target } => with_target(ServerCommand::SetScene, target),
        Command::SetProfile { target } => with_target(ServerCommand::SetProfile, target),
        Command::SetSceneCollection { target } => {
            with_target(ServerCommand::SetSceneCollection, target)
        }
        Command::SetSceneProfile { target } => with_target(ServerCommand::SetSceneProfile, target),
        Command::ClearSceneProfile => Ok(CommandPayload::simple(ServerCommand::ClearSceneProfile)),
        Command::Mute { target } => with_target(ServerCommand::Mute, target),
        Command::Unmute { target } => with_target(ServerCommand::Unmute, target),
        Command::ToggleMute { target } => with_target(ServerCommand::ToggleMute, target),

        Command::SetVolume { target, percent } => {
            // Checked before the target so an out-of-range percentage is
            // reported as such even when the target is also wrong.
            if percent > 100 {
                return Err("volume percent must be 0-100".to_string());
            }
            let target = sanitize_target_arg(&target)?;
            Ok(CommandPayload::set_volume(&target, percent))
        }

        // `dispatch_palette_command` acts on these itself and never reaches
        // here. An `Err` rather than a panic: a future caller that does reach
        // it gets a status line, not a dead TUI.
        Command::Help | Command::Quit | Command::Themes => {
            Err(format!("{cmd:?} is not a daemon command"))
        }
    }
}

fn sanitize_target_arg(value: &str) -> std::result::Result<String, String> {
    checked_name(value).map_err(|error| format!("{error}"))
}

/// How to describe a successful reply that carried no human-readable
/// `message` field.
#[derive(Clone, Copy)]
pub(super) enum ReplyStyle {
    /// Commands whose worth is that they succeeded — say so and stop.
    Acknowledge,
    /// Query commands whose worth is in the payload; a bare "ok" would throw
    /// away the very thing the user asked for, so render the payload instead.
    ShowPayload,
}

impl ReplyStyle {
    pub(super) async fn send(self, socket_path: &Path, command: ServerCommand) -> String {
        let payload = CommandPayload::simple(command);
        self.format(send_command(socket_path, payload).await, "ok")
    }

    /// Turn a daemon reply into the one status line the TUI has room for.
    fn format(self, res: Result<ServerMessage>, ok_fallback: &str) -> String {
        match res {
            Ok(ServerMessage::Response {
                ok, result, error, ..
            }) => {
                if ok {
                    self.describe_success(result.as_ref(), ok_fallback)
                } else {
                    error
                        .map(|e| format!("error [{}]: {}", e.code, e.message))
                        .unwrap_or_else(|| "error".to_string())
                }
            }
            Ok(_) => "unexpected response".to_string(),
            Err(e) => format!("error: {e}"),
        }
    }

    fn describe_success(self, result: Option<&serde_json::Value>, ok_fallback: &str) -> String {
        result
            .and_then(|v| v.get("message"))
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .or_else(|| match self {
                ReplyStyle::Acknowledge => None,
                ReplyStyle::ShowPayload => result.map(summarize_json),
            })
            .unwrap_or_else(|| ok_fallback.to_string())
    }
}

pub(super) async fn send_simple_with_target(
    socket_path: &Path,
    command: ServerCommand,
    target: &str,
) -> String {
    let target = match checked_target(target) {
        Ok(target) => target,
        Err(message) => return message,
    };

    let payload = CommandPayload::with_target(command, &target);
    ReplyStyle::Acknowledge.format(send_command(socket_path, payload).await, "ok")
}

/// Validate a target name, returning the status line to show on rejection.
fn checked_target(target: &str) -> std::result::Result<String, String> {
    sanitize_target_arg(target).map_err(|error| format!("error: invalid target: {error}"))
}

/// Most scenes one scene profile may hide.
///
/// Mirrors the cap the daemon enforces in `server::command_executor`, which is
/// the authority: it rejects a longer list outright. Checking here as well
/// turns that rejection into a message the user can act on without spending a
/// round-trip on a payload that cannot be accepted.
const MAX_HIDDEN_SCENES_PER_PROFILE: usize = 128;

/// Save a scene profile: `name`, the scenes it hides, and — when the editor
/// was opened on an existing profile — which entry this is replacing.
///
/// Every name is validated here, before anything reaches the wire, because
/// the daemon rejects the whole payload over a single unusable entry — and a
/// scene name comes from OBS, which does not share obsctl's idea of what a
/// usable name is.
///
/// `rename_from` is passed through untouched apart from that validation: which
/// entry it identifies, and whether the name has changed at all, are questions
/// the daemon answers with its own trim-and-lowercase comparison. Deciding
/// either here is how a stored name with surrounding whitespace used to be
/// read as a rename that had not happened.
pub(super) async fn send_save_scene_profile(
    socket_path: &Path,
    name: &str,
    hidden: &[String],
    rename_from: Option<&str>,
) -> String {
    let name = match checked_target(name) {
        Ok(name) => name,
        Err(message) => return message,
    };
    let rename_from = match rename_from.map(checked_target).transpose() {
        Ok(previous) => previous,
        Err(message) => return message,
    };
    if hidden.len() > MAX_HIDDEN_SCENES_PER_PROFILE {
        return format!(
            "error: a scene profile may hide at most {MAX_HIDDEN_SCENES_PER_PROFILE} scenes"
        );
    }
    let mut scenes = Vec::with_capacity(hidden.len());
    for scene in hidden {
        match sanitize_target_arg(scene) {
            Ok(scene) => scenes.push(scene),
            Err(error) => return format!("error: invalid scene name: {error}"),
        }
    }

    let payload = CommandPayload::save_scene_profile(&name, &scenes, rename_from.as_deref());
    ReplyStyle::Acknowledge.format(send_command(socket_path, payload).await, "ok")
}

pub(super) async fn send_set_volume(socket_path: &Path, target: &str, percent: u8) -> String {
    let target = match checked_target(target) {
        Ok(target) => target,
        Err(message) => return message,
    };
    if percent > 100 {
        return "error: volume percent must be 0-100".to_string();
    }

    let payload = CommandPayload::set_volume(&target, percent);
    ReplyStyle::Acknowledge.format(
        send_command(socket_path, payload).await,
        &format!("volume → {percent}%"),
    )
}

/// One-line rendering of a status payload, truncated on a char boundary so
/// it fits the single result line the palette gives us.
fn summarize_json(value: &serde_json::Value) -> String {
    const MAX_CHARS: usize = 220;
    let text = value.to_string();
    match text.char_indices().nth(MAX_CHARS) {
        Some((byte_idx, _)) => format!("{}…", &text[..byte_idx]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::command_to_payload;
    use crate::domain::command::Command;
    use crate::support::validation::MAX_TARGET_TOKEN_LENGTH;

    #[test]
    fn command_to_payload_rejects_invalid_target_values() {
        let cmd = Command::SetScene {
            target: "  \t\n  ".to_string(),
        };
        assert!(command_to_payload(cmd).is_err());

        let cmd = Command::Mute {
            target: "Mic\nInput".to_string(),
        };
        assert!(command_to_payload(cmd).is_err());
    }

    #[test]
    fn command_to_payload_sanitizes_target_whitespace() {
        let cmd = Command::ToggleMute {
            target: "  Mic  ".to_string(),
        };
        let payload = command_to_payload(cmd).unwrap();
        assert_eq!(payload.name, "toggle_mute");
        assert_eq!(
            payload.args.get("target").and_then(|v| v.as_str()).unwrap(),
            "Mic"
        );
    }

    /// The palette's two scene-profile commands, which are `set_scene_profile`
    /// and `clear_scene_profile` on the wire — never `set_profile`, which is
    /// the OBS profile.
    #[test]
    fn command_to_payload_maps_the_scene_profile_commands() {
        let payload = command_to_payload(Command::SetSceneProfile {
            target: " streaming ".to_string(),
        })
        .unwrap();
        assert_eq!(payload.name, "set_scene_profile");
        assert_eq!(
            payload.args.get("target").and_then(|v| v.as_str()).unwrap(),
            "streaming"
        );

        let payload = command_to_payload(Command::ClearSceneProfile).unwrap();
        assert_eq!(payload.name, "clear_scene_profile");
    }

    #[test]
    fn command_to_payload_rejects_excessive_target_length() {
        let cmd = Command::SetVolume {
            target: "a".repeat(MAX_TARGET_TOKEN_LENGTH + 1),
            percent: 50,
        };
        assert!(command_to_payload(cmd).is_err());
    }

    #[test]
    fn command_to_payload_rejects_volume_percent_out_of_range() {
        let cmd = Command::SetVolume {
            target: "Mic".to_string(),
            percent: 101,
        };
        assert!(command_to_payload(cmd).is_err());
    }
}
