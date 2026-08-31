//! Everything the TUI sends to the daemon, and how it renders what comes
//! back.
//!
//! The TUI is an IPC client like the CLI: it never opens a WebSocket to OBS
//! itself. Keeping the payload building, the target validation, and the
//! one-line reply rendering together means the rules about what may be put on
//! the wire are read in one place rather than beside the key bindings that
//! happen to use them.

use std::path::Path;

use rust_i18n::t;
use serde_json::Value;

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
    t!("tui.daemon.help_line", commands = commands.join(" ")).into_owned()
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
        Err(e) => {
            return PaletteOutcome::Status(t!("tui.daemon.reply_error", error = e).into_owned());
        }
    };

    match command {
        Command::Quit => PaletteOutcome::Quit,
        Command::Themes => PaletteOutcome::OpenSettings,
        Command::Help => PaletteOutcome::Status(help_line()),
        // The two scene-profile commands are answered with the TUI's own
        // sentence rather than the daemon's `message`, so that switching a
        // profile on from the command line says how many scenes it hides —
        // exactly what the picker's `a` reports for the same event. Routing
        // them through the generic path below would throw the reply's `hidden`
        // count and its `warnings` away.
        Command::SetSceneProfile { target } => {
            PaletteOutcome::Status(activate_scene_profile(socket_path, &target).await)
        }
        Command::ClearSceneProfile => {
            PaletteOutcome::Status(clear_scene_profile(socket_path).await)
        }
        command => {
            let payload = match command_to_payload(command) {
                Ok(payload) => payload,
                Err(error) => {
                    return PaletteOutcome::Status(
                        t!("tui.daemon.reply_error", error = error).into_owned(),
                    );
                }
            };
            PaletteOutcome::Status(
                ReplyStyle::Acknowledge.format(send_command(socket_path, payload).await),
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
        Command::DeleteSceneProfile { target } => {
            with_target(ServerCommand::DeleteSceneProfile, target)
        }
        Command::Mute { target } => with_target(ServerCommand::Mute, target),
        Command::Unmute { target } => with_target(ServerCommand::Unmute, target),
        Command::ToggleMute { target } => with_target(ServerCommand::ToggleMute, target),

        Command::SetVolume { target, percent } => {
            // Checked before the target so an out-of-range percentage is
            // reported as such even when the target is also wrong.
            let percent = checked_volume(percent)?;
            let target = sanitize_target_arg(&target)?;
            Ok(CommandPayload::set_volume(&target, percent))
        }

        // `dispatch_palette_command` acts on these itself and never reaches
        // here. An `Err` rather than a panic: a future caller that does reach
        // it gets a status line, not a dead TUI.
        Command::Help | Command::Quit | Command::Themes => {
            Err(t!("tui.daemon.not_a_daemon_command").into_owned())
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
    /// Query commands whose worth is in the payload; a bare acknowledgement
    /// would throw away the very thing the user asked for, so render the
    /// payload instead.
    ShowPayload,
}

impl ReplyStyle {
    pub(super) async fn send(self, socket_path: &Path, command: ServerCommand) -> String {
        let payload = CommandPayload::simple(command);
        self.format(send_command(socket_path, payload).await)
    }

    /// Turn a daemon reply into the one status line the TUI has room for.
    fn format(self, res: Result<ServerMessage>) -> String {
        self.format_with(res, &t!("tui.daemon.ok"))
    }

    /// Same as [`ReplyStyle::format`], but with the caller's own wording for a
    /// success the daemon described with neither a `message` nor a payload.
    fn format_with(self, res: Result<ServerMessage>, line: &str) -> String {
        match reply_payload(res) {
            Ok(result) => self.describe_success_with(result.as_ref(), line),
            Err(line) => line,
        }
    }

    fn describe_success_with(self, result: Option<&Value>, ok_fallback: &str) -> String {
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

    #[cfg(test)]
    fn describe_success(self, result: Option<&Value>) -> String {
        self.describe_success_with(result, &t!("tui.daemon.ok"))
    }
}

/// Split a reply into the payload the daemon sent back, or the status line
/// that says why there is none.
///
/// [`ReplyStyle::format`] renders both halves into one string; callers that
/// read the payload's own fields — the scene-profile commands below — need the
/// halves kept apart, and must report a failure the same way `format` does
/// rather than inventing a second wording for it.
fn reply_payload(res: Result<ServerMessage>) -> std::result::Result<Option<Value>, String> {
    match res {
        Ok(ServerMessage::Response {
            ok, result, error, ..
        }) => {
            if ok {
                Ok(result)
            } else {
                Err(error
                    .map(|e| {
                        t!(
                            "tui.daemon.reply_error_with_code",
                            code = e.code,
                            message = e.message
                        )
                        .into_owned()
                    })
                    .unwrap_or_else(|| t!("tui.daemon.reply_error_bare").into_owned()))
            }
        }
        Ok(_) => Err(t!("tui.daemon.unexpected_response").into_owned()),
        Err(e) => Err(t!("tui.daemon.reply_error", error = e).into_owned()),
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
    ReplyStyle::Acknowledge.format(send_command(socket_path, payload).await)
}

/// Validate a target name, returning the status line to show on rejection.
fn checked_target(target: &str) -> std::result::Result<String, String> {
    sanitize_target_arg(target)
        .map_err(|error| t!("tui.daemon.invalid_target", error = error).into_owned())
}

/// Validate a volume percentage, returning the message to show on rejection.
///
/// The daemon enforces the same 0-100 range; checking here reports the
/// mistake without spending a round-trip on a payload that cannot be
/// accepted.
fn checked_volume(percent: u8) -> std::result::Result<u8, String> {
    if percent > 100 {
        return Err(t!("tui.daemon.volume_out_of_range").into_owned());
    }
    Ok(percent)
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
) -> std::result::Result<SceneProfileReply, String> {
    let name = checked_target(name)?;
    let rename_from = rename_from.map(checked_target).transpose()?;
    if hidden.len() > MAX_HIDDEN_SCENES_PER_PROFILE {
        return Err(t!(
            "tui.daemon.too_many_hidden_scenes",
            max = MAX_HIDDEN_SCENES_PER_PROFILE.to_string()
        )
        .into_owned());
    }
    let mut scenes = Vec::with_capacity(hidden.len());
    for scene in hidden {
        match sanitize_target_arg(scene) {
            Ok(scene) => scenes.push(scene),
            Err(error) => {
                return Err(t!("tui.daemon.invalid_scene_name", error = error).into_owned());
            }
        }
    }

    let payload = CommandPayload::save_scene_profile(&name, &scenes, rename_from.as_deref());
    scene_profile_reply(send_command(socket_path, payload).await)
}

/// The parts of a scene-profile reply the TUI renders for itself.
///
/// The daemon's `message` field is one English sentence written for the CLI's
/// stdout. Beside it the daemon sends the numbers that sentence leaves out,
/// and those are what make an activation legible: a scene profile is a *deny*
/// list, so the useful thing to say is which profile is on and how many scenes
/// it is taking off the list. Reading them here means the wording can be
/// localized and can differ per call site without the wire message moving.
///
/// The warnings are kept as a count only. Their full text is already on its
/// way to the log panel: the daemon publishes each one on the `logs` topic as
/// it makes the change, and the TUI is subscribed to that topic.
pub(super) struct SceneProfileReply {
    /// How many scenes of the ones OBS has this profile takes off the list —
    /// the daemon has already checked its entries against the scene list, so
    /// this is what the user will see disappear, not what the config file
    /// lists.
    hidden: usize,
    /// Whether a save added a profile that did not exist a moment ago. Only a
    /// save reply carries this; everything else leaves it `false`.
    created: bool,
    /// Whether the profile a save wrote is the one currently in effect. A save
    /// never switches a profile on, but editing the profile that is *already*
    /// on moves the scene list immediately, and the two outcomes need
    /// different sentences. Only a save reply carries this.
    active: bool,
    /// How many config-validation warnings this change produced.
    warnings: usize,
}

impl SceneProfileReply {
    /// Read the fields out of a reply payload, treating anything missing or
    /// mistyped as absent.
    ///
    /// A reply that has grown or lost a field must not cost the user their
    /// status line — the change still happened, and saying nothing about it is
    /// the failure mode this whole stage exists to remove.
    fn parse(result: Option<&Value>) -> Self {
        let field = |key: &str| result.and_then(|value| value.get(key));
        Self {
            hidden: field("hidden")
                .and_then(Value::as_u64)
                .and_then(|count| usize::try_from(count).ok())
                .unwrap_or(0),
            created: field("created").and_then(Value::as_bool).unwrap_or(false),
            active: field("active").and_then(Value::as_bool).unwrap_or(false),
            warnings: field("warnings")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
        }
    }

    /// A profile was switched on: say which, and how much of the scene list it
    /// just took away.
    pub(super) fn activated_line(&self, name: &str) -> String {
        self.line("tui.panels.scene_profiles.activated", name)
    }

    /// A profile that already existed was edited.
    ///
    /// Which of the two sentences is the truthful one turns on whether the
    /// edited profile is the one in effect. It if is not, nothing on the
    /// dashboard moved, and saying so is what stops an unmoved scene list from
    /// reading as a save that did not work. If it *is*, the daemon re-resolved
    /// the scene list from the profile as it wrote it — the rows on screen
    /// changed as the save landed — and the old wording, which claimed the
    /// active profile was unchanged, described a different event from the one
    /// the user just watched happen.
    fn saved_line(&self, name: &str) -> String {
        let key = if self.active {
            "tui.panels.scene_profiles.saved_active"
        } else {
            "tui.panels.scene_profiles.saved"
        };
        self.line(key, name)
    }

    /// A profile was created and switched on in the one keypress.
    pub(super) fn saved_and_activated_line(&self, name: &str) -> String {
        self.line("tui.panels.scene_profiles.saved_and_activated", name)
    }

    fn line(&self, key: &'static str, name: &str) -> String {
        let sentence = t!(key, name = name, count = self.hidden.to_string()).into_owned();
        if self.warnings == 0 {
            return sentence;
        }
        let warnings = t!(
            "tui.panels.scene_profiles.warnings_suffix",
            count = self.warnings.to_string()
        );
        format!("{sentence} {warnings}")
    }
}

/// What a finished save leaves to do.
///
/// The rule lives here, in one readable place, rather than inside the
/// round-trip: a profile the user has *just built* is one they want to see
/// working, so it is switched on; an edit of a profile that already existed is
/// not, because the editor lets someone work on a profile they are not using
/// and swapping the scene list out from under them would be a surprise. The
/// daemon refuses to make that distinction — it cannot tell an edit the user
/// wants applied from one they do not — so it never activates on save and
/// hands back `created` for the client to decide with.
enum SaveFollowUp {
    /// Nothing left to send; this is the status line.
    Done(String),
    /// The profile is new — switch to it.
    Activate,
}

fn save_follow_up(name: &str, saved: &SceneProfileReply) -> SaveFollowUp {
    if saved.created {
        SaveFollowUp::Activate
    } else {
        SaveFollowUp::Done(saved.saved_line(name))
    }
}

/// Save a scene profile and, when it turns out to be a new one, switch to it.
///
/// Two round-trips on the same socket path rather than one command, because
/// activating on save is a client decision (see [`SaveFollowUp`]) and the
/// server's `save_scene_profile` deliberately does not make it.
pub(super) async fn save_and_maybe_activate_scene_profile(
    socket_path: &Path,
    name: &str,
    hidden: &[String],
    rename_from: Option<&str>,
) -> String {
    // Trimmed once up front so the three status lines below quote the name the
    // config file will hold, not whatever spacing the editor's text field was
    // left with. Both sends validate again, which costs nothing: the check is
    // idempotent.
    let name = match checked_target(name) {
        Ok(name) => name,
        Err(line) => return line,
    };
    let saved = match send_save_scene_profile(socket_path, &name, hidden, rename_from).await {
        Ok(reply) => reply,
        Err(line) => return line,
    };
    match save_follow_up(&name, &saved) {
        SaveFollowUp::Done(line) => line,
        SaveFollowUp::Activate => match send_set_scene_profile(socket_path, &name).await {
            Ok(active) => active.saved_and_activated_line(&name),
            // The save happened and stays happened; only the switch failed —
            // the config could have been edited underneath us between the two
            // commands. Reporting a plain success here would leave the user
            // waiting for a scene list that is never going to shrink.
            Err(error) => t!(
                "tui.panels.scene_profiles.activation_failed",
                name = name,
                error = error
            )
            .into_owned(),
        },
    }
}

/// Switch a scene profile on and describe what that did.
///
/// Shared by the picker's `a`, the palette's `:scene-profile <name>`, and the
/// follow-up a freshly created profile gets, so all three report the same
/// event with the same sentence.
pub(super) async fn activate_scene_profile(socket_path: &Path, name: &str) -> String {
    match send_set_scene_profile(socket_path, name).await {
        Ok(reply) => reply.activated_line(name),
        Err(line) => line,
    }
}

/// Switch scene-profile filtering off, handing the per-scene `hidden` flags
/// back their say — which is the part a user cannot guess and the line says.
pub(super) async fn clear_scene_profile(socket_path: &Path) -> String {
    let payload = CommandPayload::simple(ServerCommand::ClearSceneProfile);
    match reply_payload(send_command(socket_path, payload).await) {
        Ok(_) => t!("tui.panels.scene_profiles.cleared").into_owned(),
        Err(line) => line,
    }
}

async fn send_set_scene_profile(
    socket_path: &Path,
    name: &str,
) -> std::result::Result<SceneProfileReply, String> {
    let name = checked_target(name)?;
    let payload = CommandPayload::with_target(ServerCommand::SetSceneProfile, &name);
    scene_profile_reply(send_command(socket_path, payload).await)
}

fn scene_profile_reply(
    res: Result<ServerMessage>,
) -> std::result::Result<SceneProfileReply, String> {
    reply_payload(res).map(|result| SceneProfileReply::parse(result.as_ref()))
}

pub(super) async fn send_set_volume(socket_path: &Path, target: &str, percent: u8) -> String {
    let target = match checked_target(target) {
        Ok(target) => target,
        Err(message) => return message,
    };
    let percent = match checked_volume(percent) {
        Ok(percent) => percent,
        Err(error) => return t!("tui.daemon.reply_error", error = error).into_owned(),
    };

    let payload = CommandPayload::set_volume(&target, percent);
    ReplyStyle::Acknowledge.format_with(
        send_command(socket_path, payload).await,
        &t!("tui.daemon.volume_set", percent = percent.to_string()),
    )
}

/// One-line rendering of a status payload, truncated on a char boundary so
/// it fits the single result line the palette gives us.
fn summarize_json(value: &Value) -> String {
    const MAX_CHARS: usize = 220;
    let text = value.to_string();
    match text.char_indices().nth(MAX_CHARS) {
        Some((byte_idx, _)) => format!("{}…", &text[..byte_idx]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplyStyle, SaveFollowUp, SceneProfileReply, command_to_payload, save_follow_up};
    use crate::domain::command::Command;
    use crate::support::validation::MAX_TARGET_TOKEN_LENGTH;
    use rust_i18n::t;
    use serde_json::json;

    /// A reply with neither a `message` nor a payload is acknowledged with the
    /// translated string, never a hard-coded literal — so dropping the key
    /// fails here rather than showing English inside a translated TUI.
    #[test]
    fn acknowledge_falls_back_to_the_translated_ok_line() {
        assert_eq!(
            ReplyStyle::Acknowledge.describe_success(None),
            t!("tui.daemon.ok")
        );
    }

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

    /// The exact payload `server::command_executor::cmd_save_scene_profile`
    /// answers a creating save with.
    fn created_save_reply() -> serde_json::Value {
        json!({
            "message": "scene profile saved: streaming",
            "hidden": 6,
            "created": true,
            "renamed": false,
            "warnings": [],
        })
    }

    /// A scene profile hides scenes, so the one number worth putting on the
    /// status line is how many of them just left the list. It comes from the
    /// reply's own field, never from parsing the daemon's English sentence.
    #[test]
    fn the_status_line_names_the_profile_and_counts_what_it_hides() {
        let payload = created_save_reply();
        let reply = SceneProfileReply::parse(Some(&payload));

        let line = reply.saved_and_activated_line("streaming");
        assert!(line.contains("streaming"), "got {line:?}");
        assert!(line.contains('6'), "got {line:?}");
        assert!(
            !line.to_lowercase().contains("warning"),
            "a clean save must not mention warnings, got {line:?}"
        );
    }

    /// The `warnings` array used to be dropped on the floor, so a config the
    /// daemon had just complained about looked exactly like a clean one.
    #[test]
    fn config_warnings_are_counted_onto_the_status_line() {
        let payload = json!({
            "message": "scene profile set: streaming",
            "hidden": 6,
            "warnings": ["ui.theme is not a known theme", "obs.port is unusual"],
        });
        let reply = SceneProfileReply::parse(Some(&payload));

        let line = reply.activated_line("streaming");
        assert!(
            line.contains('6'),
            "still counts the hidden scenes: {line:?}"
        );
        assert!(line.contains('2'), "and the warnings: {line:?}");
        assert!(line.to_lowercase().contains("warning"), "got {line:?}");
    }

    /// A reply that has lost or renamed a field still has to produce a line:
    /// the change happened, and silence is the failure this reporting exists
    /// to remove.
    #[test]
    fn a_reply_without_the_expected_fields_still_produces_a_line() {
        let reply = SceneProfileReply::parse(Some(&json!({ "message": "done" })));
        assert!(!reply.created);
        let line = reply.activated_line("streaming");
        assert!(line.contains("streaming"), "got {line:?}");

        let line = SceneProfileReply::parse(None).activated_line("streaming");
        assert!(line.contains("streaming"), "got {line:?}");
    }

    /// Saving the profile that is *already* switched on re-resolves the scene
    /// list as it writes, so the dashboard moves while the status line is
    /// being drawn. The line used to say "the active profile is unchanged" for
    /// this case, which is true of the pointer and false of everything the
    /// user was looking at.
    #[test]
    fn saving_the_active_scene_profile_says_the_scene_list_just_moved() {
        let payload = json!({
            "message": "scene profile saved: streaming",
            "hidden": 2,
            "listed": 2,
            "created": false,
            "renamed": false,
            "active": true,
            "warnings": [],
        });
        let SaveFollowUp::Done(line) =
            save_follow_up("streaming", &SceneProfileReply::parse(Some(&payload)))
        else {
            panic!("a save never switches a profile on, not even the active one");
        };
        assert!(line.contains("streaming"), "got {line:?}");
        assert!(line.contains('2'), "got {line:?}");
        assert!(
            !line.contains("unchanged"),
            "the scene list did change: {line:?}"
        );

        // The same save of a profile that is *not* in effect changes nothing
        // on screen, and that is the sentence it gets.
        let mut payload = payload;
        payload["active"] = json!(false);
        let SaveFollowUp::Done(line) =
            save_follow_up("streaming", &SceneProfileReply::parse(Some(&payload)))
        else {
            panic!("editing an existing profile must not change which one is active");
        };
        assert!(
            line.contains("unchanged"),
            "an inactive profile's save moves nothing: {line:?}"
        );
    }

    /// The rule the whole create flow turns on: a profile that did not exist
    /// before the keypress gets switched on, and an edit of one that did is
    /// left alone — which is why the edit's line has to say that the active
    /// profile did not change, or an unmoved scene list reads as a failed
    /// save.
    #[test]
    fn only_a_newly_created_scene_profile_is_switched_on_after_saving() {
        let payload = created_save_reply();
        let created = SceneProfileReply::parse(Some(&payload));
        assert!(matches!(
            save_follow_up("streaming", &created),
            SaveFollowUp::Activate
        ));

        let payload = json!({
            "message": "scene profile saved: streaming",
            "hidden": 6,
            "created": false,
            "renamed": false,
            "warnings": [],
        });
        let edited = SceneProfileReply::parse(Some(&payload));
        let SaveFollowUp::Done(line) = save_follow_up("streaming", &edited) else {
            panic!("editing an existing profile must not change which one is active");
        };
        assert!(line.contains("streaming"), "got {line:?}");
        assert!(line.contains('6'), "got {line:?}");
    }
}
