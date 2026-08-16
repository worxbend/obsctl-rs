// IPC proxy command implementations.

use std::path::PathBuf;

use rust_i18n::t;
use tokio::runtime::Runtime;

use crate::{
    domain::errors::ObsctlError,
    ipc::{
        protocol::{
            CommandPayload, ErrorPayload, PublicErrorCode, ServerCommand, ServerMessage,
            exit_code_for_public_error_code, public_error_code, validate_command_name,
        },
        unix_client::IpcClient,
    },
    service::systemd_user_service::SYSTEMCTL_ENABLE_HINT,
    support::redaction::redact_message,
    support::validation::{MAX_TARGET_TOKEN_LENGTH, trim_and_validate_token_with_max_len},
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
        client.send_command(payload).await
    }

    fn run_proxy(&self, payload: CommandPayload) -> i32 {
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
                        print_json_success(result);
                    } else if let Some(v) = result {
                        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
                            println!("{msg}");
                        } else {
                            println!("{v}");
                        }
                    }
                    0
                } else {
                    self.emit_response_error(error.as_ref())
                }
            }
            Ok(ServerMessage::Event { .. }) => {
                self.emit_protocol_error(&t!("cli.client.unexpected_event"))
            }
            Err(e @ ObsctlError::ServerUnavailable { .. }) => self.emit_local_error(&e),
            Err(e) => self.emit_local_error(&e),
        }
    }

    pub fn ping(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::Ping))
    }

    pub fn status(&self) -> i32 {
        let rt = match Self::rt() {
            Ok(rt) => rt,
            Err(e) => return self.emit_local_error(&e),
        };
        match rt.block_on(self.send(CommandPayload::simple(ServerCommand::GetSnapshot))) {
            Ok(ServerMessage::Response {
                ok, result, error, ..
            }) => {
                if ok {
                    if self.json_output {
                        print_json_success(result);
                    } else if let Some(v) = result {
                        print_status_json(&v);
                    }
                    0
                } else {
                    self.emit_response_error(error.as_ref())
                }
            }
            Ok(_) => self.emit_protocol_error(&t!("cli.client.unexpected_event")),
            Err(e @ ObsctlError::ServerUnavailable { .. }) => self.emit_local_error(&e),
            Err(e) => self.emit_local_error(&e),
        }
    }

    pub fn server_status(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::GetServerStatus))
    }

    pub fn obs_status(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::GetObsStatus))
    }

    pub fn scene(&self, target: &str) -> i32 {
        self.run_target_cmd(ServerCommand::SetScene, target)
    }

    pub fn profile(&self, target: &str) -> i32 {
        self.run_target_cmd(ServerCommand::SetProfile, target)
    }

    pub fn scene_collection(&self, target: &str) -> i32 {
        self.run_target_cmd(ServerCommand::SetSceneCollection, target)
    }

    pub fn mute(&self, target: &str) -> i32 {
        self.run_target_cmd(ServerCommand::Mute, target)
    }

    pub fn unmute(&self, target: &str) -> i32 {
        self.run_target_cmd(ServerCommand::Unmute, target)
    }

    pub fn toggle_mute(&self, target: &str) -> i32 {
        self.run_target_cmd(ServerCommand::ToggleMute, target)
    }

    pub fn set_volume(&self, target: &str, percent: u8) -> i32 {
        if percent > 100 {
            return self.emit_local_error(&ObsctlError::CommandParseError(
                "percent must be 0-100".to_string(),
            ));
        }

        let target = match sanitize_target_arg(target) {
            Ok(value) => value,
            Err(error) => return self.emit_local_error(&error),
        };

        self.run_proxy(CommandPayload::set_volume(&target, percent))
    }

    pub fn reconnect(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::ReconnectObs))
    }

    pub fn shutdown_server(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::ShutdownServer))
    }

    pub fn dump_config(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::DumpConfig))
    }

    pub fn reload_config(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::ReloadConfig))
    }

    pub fn toggle_stream(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::ToggleStream))
    }

    pub fn toggle_record(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::ToggleRecord))
    }

    pub fn validate_config(&self) -> i32 {
        self.run_proxy(CommandPayload::simple(ServerCommand::ValidateConfig))
    }

    fn emit_response_error(&self, error: Option<&ErrorPayload>) -> i32 {
        let (code, msg) = error
            .map(|e| (e.code.as_str(), e.message.as_str()))
            .unwrap_or((PublicErrorCode::ServerError.as_str(), "unknown error"));
        let exit_code = map_error_code(code);
        if self.json_output {
            print_json_error(code, msg, exit_code);
        } else {
            eprintln!(
                "{}",
                t!(
                    "cli.client.error_with_code",
                    code = code,
                    message = redact_message(msg)
                )
            );
        }
        exit_code
    }

    fn emit_protocol_error(&self, message: &str) -> i32 {
        let code = PublicErrorCode::IpcProtocolError.as_str();
        let exit_code = PublicErrorCode::IpcProtocolError.exit_code();
        if self.json_output {
            print_json_error(code, message, exit_code);
        } else {
            eprintln!("{}", redact_message(message));
        }
        exit_code
    }

    fn emit_local_error(&self, error: &ObsctlError) -> i32 {
        let code = public_error_code(error);
        let exit_code = code.exit_code();
        if self.json_output {
            let message = match error {
                ObsctlError::ServerUnavailable { message, .. } => message.as_str(),
                _ => {
                    print_json_error(code.as_str(), &error.to_string(), exit_code);
                    return exit_code;
                }
            };
            print_json_error(code.as_str(), message, exit_code);
        } else if matches!(error, ObsctlError::ServerUnavailable { .. }) {
            eprintln!("{}", redact_message(error.to_string()));
        } else {
            eprintln!(
                "{}",
                t!("common.error", message = redact_message(error.to_string()))
            );
        }
        exit_code
    }

    fn run_target_cmd(&self, command: ServerCommand, target: &str) -> i32 {
        let target = match sanitize_target_arg(target) {
            Ok(value) => value,
            Err(error) => return self.emit_local_error(&error),
        };
        self.run_proxy(CommandPayload::with_target(command, &target))
    }
}

fn sanitize_target_arg(value: &str) -> Result<String, ObsctlError> {
    trim_and_validate_token_with_max_len(value, MAX_TARGET_TOKEN_LENGTH)
        .map_err(|error| ObsctlError::CommandParseError(format!("target {error}")))
}

fn map_error_code(code: &str) -> i32 {
    exit_code_for_public_error_code(code)
}

fn print_json_success(result: Option<serde_json::Value>) {
    let envelope = serde_json::json!({
        "ok": true,
        "result": result.unwrap_or(serde_json::Value::Null),
        "error": serde_json::Value::Null,
        "exit_code": 0,
    });
    print_json_envelope(
        &envelope,
        "{\"ok\":false,\"result\":null,\"error\":{\"code\":\"internal\",\"message\":\"failed to encode JSON response\"},\"exit_code\":1}",
    );
}

fn print_json_error(code: &str, message: &str, exit_code: i32) {
    let safe_message = redact_message(message);
    let envelope = serde_json::json!({
        "ok": false,
        "result": serde_json::Value::Null,
        "error": {
            "code": code,
            "message": safe_message,
        },
        "exit_code": exit_code,
    });
    let fallback = r#"{"ok":false,"result":null,"error":{"code":"internal","message":"failed to encode JSON output"},"exit_code":1}"#;
    print_json_envelope(&envelope, fallback);
}

fn print_status_json(v: &serde_json::Value) {
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            println!("{k}: {val}");
        }
    } else {
        println!("{v}");
    }
}

fn print_json_envelope(envelope: &serde_json::Value, fallback: &str) {
    match serde_json::to_string(envelope) {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!(
                "{}",
                t!(
                    "common.warning",
                    message = t!("cli.client.json_encode_warning", error = error)
                )
            );
            println!("{fallback}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::PublicErrorCode;
    use std::path::PathBuf;

    #[test]
    fn proxy_error_code_mapping_uses_public_contract() {
        for code in PublicErrorCode::ALL {
            assert_eq!(map_error_code(code.as_str()), code.exit_code(), "{code}");
        }

        assert_eq!(map_error_code("UNKNOWN_CODE"), 1);
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

        let exit_code = ctx.run_proxy(CommandPayload {
            name: "bad command".to_string(),
            args: serde_json::Value::Null,
        });

        assert_eq!(exit_code, PublicErrorCode::CommandParseError.exit_code(),);
    }

    #[test]
    fn set_volume_rejects_percent_out_of_range_without_server() {
        let ctx = ProxyCtx {
            socket_path: PathBuf::from("/tmp/obsctl-test.sock"),
            json_output: false,
        };

        assert_eq!(ctx.set_volume("Mic", 101), 5);
        assert_eq!(ctx.set_volume("Mic", 255), 5);
    }

    #[test]
    fn target_cmd_rejects_invalid_target_without_server() {
        let ctx = ProxyCtx {
            socket_path: PathBuf::from("/tmp/obsctl-test.sock"),
            json_output: false,
        };

        assert_eq!(ctx.run_target_cmd(ServerCommand::Mute, "Bad\nTarget"), 5);
        assert_eq!(ctx.run_target_cmd(ServerCommand::SetScene, "   "), 5);
    }
}
