// IPC proxy command implementations.

use std::path::PathBuf;

use rust_i18n::t;
use tokio::runtime::Runtime;

use crate::{
    domain::{errors::ObsctlError, names::checked_name},
    ipc::{
        protocol::{
            CommandPayload, ErrorPayload, PublicErrorCode, ServerMessage,
            exit_code_for_public_error_code, public_error_code, validate_command_name,
        },
        unix_client::{IpcClient, send_command_within_timeout},
    },
    service::systemd_user_service::SYSTEMCTL_ENABLE_HINT,
    support::redaction::redact_message,
};

fn server_unavailable_hint() -> String {
    format!(
        "{}\n{}\n  obsctl server --headless\n{}\n  obsctl service install\n  {}",
        t!("cli.client.server_unavailable_intro"),
        t!("cli.client.server_unavailable_start"),
        t!("cli.client.server_unavailable_or_install"),
        SYSTEMCTL_ENABLE_HINT
    )
}

pub struct ProxyCtx {
    pub socket_path: PathBuf,
    pub json_output: bool,
}

impl ProxyCtx {
    fn rt() -> Result<Runtime, ObsctlError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(ObsctlError::Io)
    }

    async fn send(&self, payload: CommandPayload) -> Result<ServerMessage, ObsctlError> {
        let mut client = IpcClient::connect(&self.socket_path).await.map_err(|_| {
            ObsctlError::ServerUnavailable {
                socket_path: self.socket_path.display().to_string(),
                message: server_unavailable_hint(),
            }
        })?;
        send_command_within_timeout(&mut client, payload).await
    }

    /// Send `payload`, then hand a successful result to `render`.
    ///
    /// Everything except the rendering — command-name validation, building the
    /// runtime, the `--json` envelope, protocol errors, and the exit-code
    /// mapping — is identical for every proxy command, so only the one varying
    /// step is a parameter.
    pub(crate) fn run_proxy_with(
        &self,
        payload: CommandPayload,
        render: fn(&serde_json::Value),
    ) -> i32 {
        // Every payload reaching here is built from a `ServerCommand` enum by
        // `cli::router::proxy_payload`, so in production this check cannot
        // fail. It stays as boundary defence: `CommandPayload` is a plain
        // struct anyone could fill in with an arbitrary name, and a malformed
        // name must be refused before a connection is opened rather than after.
        if let Err(error) = validate_command_name(&payload.name) {
            return self.emit_local_error(&ObsctlError::CommandParseError(format!(
                "invalid command name: {error}"
            )));
        }

        let rt = match Self::rt() {
            Ok(rt) => rt,
            Err(e) => return self.emit_local_error(&e),
        };

        match rt.block_on(self.send(payload)) {
            Ok(ServerMessage::Response {
                ok, result, error, ..
            }) => {
                if ok {
                    if self.json_output {
                        print_envelope(true, result, None, 0);
                    } else if let Some(v) = result {
                        render(&v);
                    }
                    0
                } else {
                    self.emit_response_error(error.as_ref())
                }
            }
            Ok(ServerMessage::Event { .. }) => {
                self.emit_protocol_error(&t!("cli.client.unexpected_event"))
            }
            Err(e) => self.emit_local_error(&e),
        }
    }

    /// Report `outcome` in whichever form the invocation asked for, and hand
    /// back its exit code.
    ///
    /// This is the single place that decides between the `--json` envelope and
    /// a line on stderr, applies redaction, and returns the process exit code.
    /// Three near-identical copies of this branch used to live in the three
    /// `emit_*` functions below; they now only *describe* the failure and leave
    /// the reporting to this one.
    fn report(&self, outcome: Outcome) -> i32 {
        if self.json_output {
            print_envelope(
                false,
                None,
                Some((outcome.code.as_str(), outcome.message.as_str())),
                outcome.exit_code,
            );
        } else if outcome.bare_text {
            eprintln!("{}", redact_message(&outcome.message));
        } else if outcome.show_code {
            eprintln!(
                "{}",
                t!(
                    "cli.client.error_with_code",
                    code = outcome.code,
                    message = redact_message(&outcome.message)
                )
            );
        } else {
            eprintln!(
                "{}",
                t!("common.error", message = redact_message(&outcome.message))
            );
        }
        outcome.exit_code
    }

    fn emit_response_error(&self, error: Option<&ErrorPayload>) -> i32 {
        let (code, message) = error
            .map(|e| (e.code.as_str(), e.message.as_str()))
            .unwrap_or((PublicErrorCode::ServerError.as_str(), "unknown error"));
        self.report(Outcome {
            exit_code: exit_code_for_public_error_code(code),
            code: code.to_string(),
            message: message.to_string(),
            bare_text: false,
            show_code: true,
        })
    }

    fn emit_protocol_error(&self, message: &str) -> i32 {
        self.report(Outcome {
            code: PublicErrorCode::IpcProtocolError.as_str().to_string(),
            message: message.to_string(),
            exit_code: PublicErrorCode::IpcProtocolError.exit_code(),
            // The protocol message is already a finished sentence, so it is
            // printed as-is rather than wrapped in "error: ...".
            bare_text: true,
            show_code: false,
        })
    }

    pub(crate) fn emit_local_error(&self, error: &ObsctlError) -> i32 {
        let code = public_error_code(error);
        // "Server unavailable" carries a multi-line hint telling the user how to
        // start the daemon. That hint is the whole message, so it is printed
        // plain instead of being wrapped in "error: ...". The old code decided
        // this twice — once inside the `--json` arm and again with a `matches!`
        // in the text arm; here it is one flag, set once.
        let bare_text = matches!(error, ObsctlError::ServerUnavailable { .. });
        let message = match error {
            ObsctlError::ServerUnavailable { message, .. } => message.clone(),
            other => other.to_string(),
        };
        self.report(Outcome {
            code: code.as_str().to_string(),
            message,
            exit_code: code.exit_code(),
            bare_text,
            show_code: false,
        })
    }
}

/// A failure ready to be reported, independent of how it will be shown.
///
/// Collecting the four things every failure needs — a public error code, a
/// message, the process exit code, and how the plain-text form should be
/// worded — into one value is what lets `ProxyCtx::report` own the `--json`
/// versus stderr decision instead of each caller repeating it.
struct Outcome {
    /// Public IPC error code, e.g. `OBS_UNAVAILABLE`. Appears verbatim in the
    /// `--json` envelope, which is a public contract.
    code: String,
    /// Human-readable text. Redacted by `report`, not by the constructors, so
    /// redaction cannot be forgotten at one of the call sites.
    message: String,
    exit_code: i32,
    /// Print the message on its own, with no `error: ` prefix — for messages
    /// that are already a complete explanation.
    bare_text: bool,
    /// Print the message prefixed with its error code, the form used for
    /// failures the daemon reported.
    show_code: bool,
}

/// The default rendering for a successful proxy command: the daemon's
/// human-readable `message`, or the raw result when a command has none.
pub(crate) fn print_result_message(result: &serde_json::Value) {
    match result.get("message").and_then(|m| m.as_str()) {
        Some(message) => println!("{message}"),
        None => println!("{result}"),
    }
}

/// Trim a target name from the command line and refuse the ones that could not
/// name anything in OBS (blank, over-long, or containing control characters).
///
/// Applied by `cli::router::proxy_payload` before a payload is built, so a bad
/// target is rejected without opening a connection to the daemon.
pub(crate) fn sanitize_target_arg(value: &str) -> Result<String, ObsctlError> {
    checked_name(value).map_err(|error| ObsctlError::CommandParseError(format!("target {error}")))
}

/// What `--json` prints when the envelope itself cannot be serialized.
///
/// Serializing the envelope can only fail in ways that would leave the caller
/// with no JSON at all, so a literal well-formed envelope is printed instead.
/// There used to be two of these constants — the success and the error path
/// each carried their own — and they disagreed about the wording
/// ("failed to encode JSON response" versus "failed to encode JSON output").
/// The surviving wording is the one that matches the `cli.client.json_encode_warning`
/// locale string printed alongside it.
const JSON_ENCODE_FALLBACK: &str = r#"{"ok":false,"result":null,"error":{"code":"internal","message":"failed to encode JSON output"},"exit_code":1}"#;

/// Print the one `--json` envelope shape: `{ok, result, error, exit_code}`.
///
/// Success and failure differ only in which of `result` and `error` is filled
/// in, so they share a single builder — the field names, their order, and the
/// null-versus-value choice are a public contract and must not drift apart.
fn print_envelope(
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<(&str, &str)>,
    exit_code: i32,
) {
    let error = match error {
        Some((code, message)) => serde_json::json!({
            "code": code,
            "message": redact_message(message),
        }),
        None => serde_json::Value::Null,
    };
    let envelope = serde_json::json!({
        "ok": ok,
        "result": result.unwrap_or(serde_json::Value::Null),
        "error": error,
        "exit_code": exit_code,
    });

    match serde_json::to_string(&envelope) {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!(
                "{}",
                t!(
                    "common.warning",
                    message = t!("cli.client.json_encode_warning", error = error)
                )
            );
            println!("{JSON_ENCODE_FALLBACK}");
        }
    }
}

pub(crate) fn print_status_json(v: &serde_json::Value) {
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            println!("{k}: {val}");
        }
    } else {
        println!("{v}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::PublicErrorCode;
    use crate::support::validation::MAX_TARGET_TOKEN_LENGTH;
    use std::path::PathBuf;

    #[test]
    fn proxy_error_code_mapping_uses_public_contract() {
        for code in PublicErrorCode::ALL {
            assert_eq!(
                exit_code_for_public_error_code(code.as_str()),
                code.exit_code(),
                "{code}"
            );
        }

        assert_eq!(exit_code_for_public_error_code("UNKNOWN_CODE"), 1);
    }

    #[test]
    fn sanitize_target_arg_trimmed() {
        assert_eq!(sanitize_target_arg(" Mic ").unwrap(), "Mic");
        assert!(sanitize_target_arg("   ").is_err());
        assert!(sanitize_target_arg("a\tb").is_err());
    }

    #[test]
    fn sanitize_target_arg_rejects_excessive_length() {
        assert!(sanitize_target_arg(&"a".repeat(MAX_TARGET_TOKEN_LENGTH + 1)).is_err());
    }

    #[test]
    fn sanitize_target_arg_rejects_control_characters() {
        assert!(sanitize_target_arg("Mic\n").is_err());
        assert!(sanitize_target_arg("\tMic").is_err());
    }

    #[test]
    fn run_proxy_rejects_invalid_command_name_without_connect() {
        let ctx = ProxyCtx {
            socket_path: PathBuf::from("/tmp/obsctl-invalid-command-name.sock"),
            json_output: false,
        };

        let exit_code = ctx.run_proxy_with(
            CommandPayload {
                name: "bad command".to_string(),
                args: serde_json::Value::Null,
            },
            print_result_message,
        );

        assert_eq!(exit_code, PublicErrorCode::CommandParseError.exit_code(),);
    }
}
