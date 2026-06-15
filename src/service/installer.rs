// Service install/uninstall/start/stop/restart/status actions using a CommandRunner.
use std::path::{Path, PathBuf};

use super::systemd_user_service::{CommandRunner, unit_file_content};
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;

pub struct ServiceInstaller<'a> {
    runner: &'a dyn CommandRunner,
    unit_path: PathBuf,
}

impl<'a> ServiceInstaller<'a> {
    pub fn new(runner: &'a dyn CommandRunner, unit_path: PathBuf) -> Self {
        Self { runner, unit_path }
    }

    pub fn install(&self, exec_path: &Path) -> Result<()> {
        if let Some(parent) = self.unit_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        }
        let content = unit_file_content(exec_path);
        std::fs::write(&self.unit_path, content)
            .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        self.daemon_reload()?;
        Ok(())
    }

    pub fn uninstall(&self) -> Result<()> {
        if self.unit_path.exists() {
            std::fs::remove_file(&self.unit_path)
                .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        }
        self.daemon_reload()?;
        Ok(())
    }

    pub fn start(&self) -> Result<String> {
        self.runner
            .run("systemctl", &["--user", "start", "obsctl.service"])
    }

    pub fn stop(&self) -> Result<String> {
        self.runner
            .run("systemctl", &["--user", "stop", "obsctl.service"])
    }

    pub fn restart(&self) -> Result<String> {
        self.runner
            .run("systemctl", &["--user", "restart", "obsctl.service"])
    }

    pub fn status(&self) -> Result<String> {
        self.runner
            .run("systemctl", &["--user", "status", "obsctl.service"])
    }

    fn daemon_reload(&self) -> Result<()> {
        self.runner
            .run("systemctl", &["--user", "daemon-reload"])
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::service::systemd_user_service::CommandRunner;

    struct FakeRunner {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        response: String,
        fail: bool,
    }

    impl FakeRunner {
        fn new(response: &str) -> Self {
            Self {
                calls: RefCell::new(vec![]),
                response: response.to_string(),
                fail: false,
            }
        }

        fn failing() -> Self {
            Self {
                calls: RefCell::new(vec![]),
                response: String::new(),
                fail: true,
            }
        }

        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.borrow().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> crate::domain::result::Result<String> {
            self.calls.borrow_mut().push((
                program.to_string(),
                args.iter().map(|s| s.to_string()).collect(),
            ));
            if self.fail {
                Err(ObsctlError::ServiceInstallFailed(
                    "fake failure".to_string(),
                ))
            } else {
                Ok(self.response.clone())
            }
        }
    }

    fn make_installer<'a>(runner: &'a FakeRunner, dir: &std::path::Path) -> ServiceInstaller<'a> {
        let unit_path = dir.join("systemd/user/obsctl.service");
        ServiceInstaller::new(runner, unit_path)
    }

    #[test]
    fn install_writes_unit_file_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let exec = std::path::Path::new("/usr/local/bin/obsctl");

        installer.install(exec).unwrap();

        let unit_path = dir.path().join("systemd/user/obsctl.service");
        assert!(unit_path.exists());
        let content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(content.contains("/usr/local/bin/obsctl server --headless"));
        assert!(content.contains("[Unit]"));
        assert!(!content.contains("sudo"));

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "systemctl");
        assert!(calls[0].1.contains(&"daemon-reload".to_string()));
    }

    #[test]
    fn uninstall_removes_unit_file_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let exec = std::path::Path::new("/usr/local/bin/obsctl");

        // First install.
        installer.install(exec).unwrap();
        assert!(dir.path().join("systemd/user/obsctl.service").exists());

        // Then uninstall.
        installer.uninstall().unwrap();
        assert!(!dir.path().join("systemd/user/obsctl.service").exists());

        let calls = runner.calls();
        // Two daemon-reload calls: one from install, one from uninstall.
        assert_eq!(calls.len(), 2);
        assert!(calls[1].1.contains(&"daemon-reload".to_string()));
    }

    #[test]
    fn uninstall_is_noop_when_unit_missing() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        // No install first — uninstall should still succeed.
        installer.uninstall().unwrap();
    }

    #[test]
    fn start_delegates_to_systemctl() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("started");
        let installer = make_installer(&runner, dir.path());

        let out = installer.start().unwrap();
        assert_eq!(out, "started");

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains(&"start".to_string()));
        assert!(calls[0].1.contains(&"obsctl.service".to_string()));
        assert!(calls[0].1.contains(&"--user".to_string()));
    }

    #[test]
    fn stop_delegates_to_systemctl() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("stopped");
        let installer = make_installer(&runner, dir.path());

        installer.stop().unwrap();
        let calls = runner.calls();
        assert!(calls[0].1.contains(&"stop".to_string()));
    }

    #[test]
    fn restart_delegates_to_systemctl() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("restarted");
        let installer = make_installer(&runner, dir.path());

        installer.restart().unwrap();
        let calls = runner.calls();
        assert!(calls[0].1.contains(&"restart".to_string()));
    }

    #[test]
    fn status_delegates_to_systemctl() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("active (running)");
        let installer = make_installer(&runner, dir.path());

        let out = installer.status().unwrap();
        assert_eq!(out, "active (running)");
        let calls = runner.calls();
        assert!(calls[0].1.contains(&"status".to_string()));
    }

    #[test]
    fn install_propagates_daemon_reload_failure() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::failing();
        let installer = make_installer(&runner, dir.path());
        let exec = std::path::Path::new("/usr/local/bin/obsctl");
        assert!(installer.install(exec).is_err());
    }
}
