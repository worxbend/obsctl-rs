use std::path::{Path, PathBuf};

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

pub fn unit_file_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("systemd/user/obsctl.service"))
}

pub fn unit_file_content(exec_path: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=obsctl OBS WebSocket control daemon\n\
         After=graphical-session.target\n\
         Wants=graphical-session.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec} server --headless\n\
         Restart=always\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exec = exec_path.display()
    )
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String>;
}

pub struct SystemctlRunner;

impl CommandRunner for SystemctlRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ObsctlError::ServiceInstallFailed(format!(
                "{program} {:?} failed: {stderr}",
                args
            )));
        }
        Ok(stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_file_contains_exec_path() {
        let content = unit_file_content(Path::new("/usr/local/bin/obsctl"));
        assert!(content.contains("/usr/local/bin/obsctl server --headless"));
        assert!(content.contains("[Unit]"));
        assert!(content.contains("[Service]"));
        assert!(content.contains("[Install]"));
        assert!(content.contains("WantedBy=default.target"));
        assert!(!content.contains("sudo"));
    }
}
