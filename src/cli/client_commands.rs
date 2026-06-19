// IPC proxy command implementations.

use std::path::PathBuf;

use tokio::runtime::Runtime;

use crate::{
    domain::errors::ObsctlError,
    ipc::{
        protocol::{
            CommandPayload, ErrorPayload, PublicErrorCode, ServerMessage,
            exit_code_for_public_error_code, public_error_code, redacted_message,
        },
        unix_client::IpcClient,
    },
};

const SERVER_UNAVAILABLE_HINT: &str = "\
obsctl server is not running.
Start it with:
  obsctl server --headless
Or install the service:
  obsctl service install
  systemctl --user enable --now obsctl.service";

pub struct ProxyCtx {
    pub socket_path: PathBuf,
    pub json_output: bool,
}

impl ProxyCtx {
    fn rt() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    async fn send(&self, payload: CommandPayload) -> Result<ServerMessage, ObsctlError> {
        let mut client = IpcClient::connect(&self.socket_path).await.map_err(|_| {
            ObsctlError::ServerUnavailable {
                socket_path: self.socket_path.display().to_string(),
                message: SERVER_UNAVAILABLE_HINT.to_string(),
            }
        })?;
        client.send_command(payload).await
    }

    fn run_proxy(&self, payload: CommandPayload) -> i32 {
        let rt = Self::rt();
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
                self.emit_protocol_error("unexpected event instead of response")
            }
            Err(e @ ObsctlError::ServerUnavailable { .. }) => self.emit_local_error(&e),
            Err(e) => self.emit_local_error(&e),
        }
    }

    pub fn ping(&self) -> i32 {
        self.run_proxy(simple_cmd("ping"))
    }

    pub fn status(&self) -> i32 {
        let rt = Self::rt();
        match rt.block_on(self.send(simple_cmd("get_snapshot"))) {
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
            Ok(_) => self.emit_protocol_error("unexpected event instead of response"),
            Err(e @ ObsctlError::ServerUnavailable { .. }) => self.emit_local_error(&e),
            Err(e) => self.emit_local_error(&e),
        }
    }

    pub fn server_status(&self) -> i32 {
        self.run_proxy(simple_cmd("get_server_status"))
    }

    pub fn obs_status(&self) -> i32 {
        self.run_proxy(simple_cmd("get_obs_status"))
    }

    pub fn scene(&self, target: &str) -> i32 {
        self.run_proxy(target_cmd("set_scene", target))
    }

    pub fn mute(&self, target: &str) -> i32 {
        self.run_proxy(target_cmd("mute", target))
    }

    pub fn unmute(&self, target: &str) -> i32 {
        self.run_proxy(target_cmd("unmute", target))
    }

    pub fn toggle_mute(&self, target: &str) -> i32 {
        self.run_proxy(target_cmd("toggle_mute", target))
    }

    pub fn set_volume(&self, target: &str, percent: u8) -> i32 {
        let args = serde_json::json!({ "target": target, "percent": percent });
        self.run_proxy(CommandPayload {
            name: "set_volume".into(),
            args,
        })
    }

    pub fn reconnect(&self) -> i32 {
        self.run_proxy(simple_cmd("reconnect_obs"))
    }

    pub fn shutdown_server(&self) -> i32 {
        self.run_proxy(simple_cmd("shutdown_server"))
    }

    pub fn dump_config(&self) -> i32 {
        self.run_proxy(simple_cmd("dump_config"))
    }

    pub fn reload_config(&self) -> i32 {
        self.run_proxy(simple_cmd("reload_config"))
    }

    pub fn validate_config(&self) -> i32 {
        self.run_proxy(simple_cmd("validate_config"))
    }

    fn emit_response_error(&self, error: Option<&ErrorPayload>) -> i32 {
        let (code, msg) = error
            .map(|e| (e.code.as_str(), e.message.as_str()))
            .unwrap_or((PublicErrorCode::ServerError.as_str(), "unknown error"));
        let exit_code = map_error_code(code);
        if self.json_output {
            print_json_error(code, msg, exit_code);
        } else {
            eprintln!("error [{code}]: {msg}");
        }
        exit_code
    }

    fn emit_protocol_error(&self, message: &str) -> i32 {
        let code = PublicErrorCode::IpcProtocolError.as_str();
        let exit_code = PublicErrorCode::IpcProtocolError.exit_code();
        if self.json_output {
            print_json_error(code, message, exit_code);
        } else {
            eprintln!("{message}");
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
            eprintln!("{error}");
        } else {
            eprintln!("error: {error}");
        }
        exit_code
    }
}

fn simple_cmd(name: &str) -> CommandPayload {
    CommandPayload {
        name: name.into(),
        args: serde_json::Value::Null,
    }
}

fn target_cmd(name: &str, target: &str) -> CommandPayload {
    CommandPayload {
        name: name.into(),
        args: serde_json::json!({ "target": target }),
    }
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
    println!("{}", serde_json::to_string(&envelope).unwrap_or_default());
}

fn print_json_error(code: &str, message: &str, exit_code: i32) {
    let safe_message = redacted_message(message);
    let envelope = serde_json::json!({
        "ok": false,
        "result": serde_json::Value::Null,
        "error": {
            "code": code,
            "message": safe_message,
        },
        "exit_code": exit_code,
    });
    println!("{}", serde_json::to_string(&envelope).unwrap_or_default());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::protocol::PublicErrorCode;

    #[test]
    fn proxy_error_code_mapping_uses_public_contract() {
        for code in PublicErrorCode::ALL {
            assert_eq!(map_error_code(code.as_str()), code.exit_code(), "{code}");
        }

        assert_eq!(map_error_code("UNKNOWN_CODE"), 1);
    }
}
