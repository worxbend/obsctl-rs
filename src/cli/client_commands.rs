// IPC proxy command implementations.

use std::path::PathBuf;

use tokio::runtime::Runtime;

use crate::{
    domain::errors::ObsctlError,
    ipc::{
        protocol::{CommandPayload, ServerMessage},
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
                    if let Some(v) = result {
                        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
                            println!("{msg}");
                        } else {
                            println!("{v}");
                        }
                    }
                    0
                } else {
                    let (code, msg) = error
                        .map(|e| (e.code, e.message))
                        .unwrap_or_else(|| ("ERROR".into(), "unknown error".into()));
                    eprintln!("error [{code}]: {msg}");
                    map_error_code(&code)
                }
            }
            Ok(ServerMessage::Event { .. }) => {
                eprintln!("unexpected event instead of response");
                6
            }
            Err(e @ ObsctlError::ServerUnavailable { .. }) => {
                eprintln!("{e}");
                3
            }
            Err(e) => {
                eprintln!("error: {e}");
                e.exit_code()
            }
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
                    if let Some(v) = result {
                        print_status_json(&v);
                    }
                    0
                } else {
                    let (code, msg) = error
                        .map(|e| (e.code, e.message))
                        .unwrap_or_else(|| ("ERROR".into(), "unknown error".into()));
                    eprintln!("error [{code}]: {msg}");
                    map_error_code(&code)
                }
            }
            Ok(_) => 6,
            Err(e @ ObsctlError::ServerUnavailable { .. }) => {
                eprintln!("{e}");
                3
            }
            Err(e) => {
                eprintln!("error: {e}");
                e.exit_code()
            }
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
    match code {
        "OBS_UNAVAILABLE" | "REQUEST_TIMEOUT" | "SCENE_NOT_FOUND" | "AUDIO_INPUT_NOT_FOUND" => 4,
        "COMMAND_PARSE_ERROR" => 5,
        "IPC_PROTOCOL_ERROR" => 6,
        "SHUTDOWN_DISABLED" => 1,
        _ => 1,
    }
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
