// Service install/uninstall/start/stop/restart/status actions using a CommandRunner.
use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::systemd_user_service::{CommandRunner, unit_file_content};
use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::support::fs;
use crate::support::validation::validate_no_control_characters;

pub struct ServiceInstaller<'a> {
    runner: &'a dyn CommandRunner,
    unit_path: PathBuf,
}

impl<'a> ServiceInstaller<'a> {
    pub fn new(runner: &'a dyn CommandRunner, unit_path: PathBuf) -> Self {
        Self { runner, unit_path }
    }

    pub fn install(&self, exec_path: &Path) -> Result<()> {
        let exec_path = validate_service_exec_path(exec_path)?;

        fs::ensure_private_parent(&self.unit_path)
            .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        fs::ensure_path_not_symlink(&self.unit_path).map_err(|_| {
            ObsctlError::ServiceInstallFailed(
                "refusing to overwrite symlink at service unit path".to_string(),
            )
        })?;

        let content = unit_file_content(&exec_path);
        fs::write_atomic_with_temp_file(&self.unit_path, "obsctl-systemd", 0o600, true, |tmp| {
            tmp.write_all(content.as_bytes())?;
            tmp.flush()?;
            tmp.sync_all()?;
            Ok(())
        })
        .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        self.daemon_reload()?;
        Ok(())
    }

    pub fn uninstall(&self) -> Result<()> {
        match fs::remove_with_type_guard(
            &self.unit_path,
            |metadata| metadata.is_file(),
            "regular file",
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ObsctlError::ServiceInstallFailed(error.to_string()));
            }
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

pub fn validate_service_exec_path(exec_path: &Path) -> Result<PathBuf> {
    let exec_text = exec_path.to_string_lossy();
    validate_no_control_characters(&exec_text).map_err(|_| {
        ObsctlError::ServiceInstallFailed(
            "service executable path contains control characters".to_string(),
        )
    })?;

    if !exec_path.is_absolute() {
        return Err(ObsctlError::ServiceInstallFailed(
            "service executable path must be an absolute path".to_string(),
        ));
    }

    fs::ensure_path_not_symlink(exec_path).map_err(|_| {
        ObsctlError::ServiceInstallFailed(
            "service executable path resolves to a symbolic link, refusing to install service"
                .to_string(),
        )
    })?;
    let metadata = fs::ensure_regular_file_with_metadata(exec_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ObsctlError::ServiceInstallFailed(format!(
                "service executable path does not exist: {}",
                exec_text
            ))
        } else {
            ObsctlError::ServiceInstallFailed(format!(
                "service executable path is not a regular executable file: {} ({error})",
                exec_text
            ))
        }
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        if mode & 0o111 == 0 {
            return Err(ObsctlError::ServiceInstallFailed(format!(
                "service executable is not executable: {}",
                exec_text
            )));
        }
        if mode & 0o022 != 0 {
            return Err(ObsctlError::ServiceInstallFailed(format!(
                "service executable is writable by group or other: {}",
                exec_text
            )));
        }
        if mode & 0o7000 != 0 {
            return Err(ObsctlError::ServiceInstallFailed(format!(
                "service executable has insecure special mode bits: {}",
                exec_text
            )));
        }
    }

    Ok(exec_path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

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

    fn make_exec_file(path: &std::path::Path) {
        use crate::support::fs::secure_permissions;
        use std::io::Write;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"#!/bin/sh\necho test\n").unwrap();
        secure_permissions(path, 0o755).unwrap();
    }

    #[test]
    fn install_writes_unit_file_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let exec = dir.path().join("obsctl");
        make_exec_file(&exec);

        installer.install(&exec).unwrap();

        let unit_path = dir.path().join("systemd/user/obsctl.service");
        assert!(unit_path.exists());
        let content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(content.contains(&format!(r#""{}" server --headless"#, exec.display())));
        assert!(content.contains("[Unit]"));
        assert!(!content.contains("sudo"));

        let calls = runner.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "systemctl");
        assert!(calls[0].1.contains(&"daemon-reload".to_string()));
    }

    #[test]
    fn install_writes_quoted_exec_path() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let exec = dir.path().join("My App/obsctl");
        make_exec_file(&exec);

        installer.install(&exec).unwrap();

        let unit_path = dir.path().join("systemd/user/obsctl.service");
        let content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(content.contains(&format!(r#""{}" server --headless"#, exec.display())));
        assert!(content.contains("server --headless"));
    }

    #[test]
    fn install_replaces_existing_unit_file() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let unit_dir = dir.path().join("systemd/user");
        std::fs::create_dir_all(&unit_dir).unwrap();
        std::fs::write(unit_dir.join("obsctl.service"), "stale unit content").unwrap();

        let exec = dir.path().join("My App/obsctl");
        make_exec_file(&exec);
        installer.install(&exec).unwrap();

        let unit_path = unit_dir.join("obsctl.service");
        let content = std::fs::read_to_string(&unit_path).unwrap();
        assert!(content.contains(&format!(r#""{}" server --headless"#, exec.display())));
        assert!(!content.contains("stale unit content"));
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_control_characters_in_exec_path() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());

        let mut name = b"obsctl".to_vec();
        name.extend(b"\n-bad");
        let file = dir
            .path()
            .join(std::path::PathBuf::from(OsString::from_vec(name)));
        make_exec_file(&file);
        let err = installer.install(&file).unwrap_err();
        assert!(err.to_string().contains("contains control characters"));
        assert!(runner.calls().is_empty());
        assert!(!dir.path().join("systemd/user/obsctl.service").exists());
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_symlinked_exec_path() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());

        let real = dir.path().join("obsctl-real");
        make_exec_file(&real);
        let link = dir.path().join("obsctl-link");
        symlink(&real, &link).unwrap();

        let err = installer.install(&link).unwrap_err();
        assert!(err.to_string().contains("symbolic link"));
        assert!(real.exists());
        assert!(!dir.path().join("systemd/user/obsctl.service").exists());
        let calls = runner.calls();
        assert!(calls.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_relative_exec_path() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let exec = std::path::Path::new("relative/path");
        let err = installer.install(exec).unwrap_err();
        assert!(err.to_string().contains("must be an absolute path"));
        assert!(runner.calls().is_empty());
        assert!(!dir.path().join("systemd/user/obsctl.service").exists());
    }

    #[test]
    fn uninstall_removes_unit_file_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let installer = make_installer(&runner, dir.path());
        let exec = dir.path().join("obsctl");
        make_exec_file(&exec);

        // First install.
        installer.install(&exec).unwrap();
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
        let exec = dir.path().join("obsctl");
        make_exec_file(&exec);
        assert!(installer.install(&exec).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn install_rejects_symlinked_unit_path() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let exec = dir.path().join("obsctl");
        make_exec_file(&exec);

        let real_file = dir.path().join("systemd/user/real-obsctl.service");
        std::fs::create_dir_all(real_file.parent().unwrap()).unwrap();
        std::fs::write(&real_file, b"[Unit]").unwrap();

        let link = dir.path().join("systemd/user/obsctl.service");
        symlink(&real_file, &link).unwrap();

        let installer = ServiceInstaller::new(&runner, link);
        let err = installer.install(&exec).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite symlink"));
        assert!(real_file.exists());
        let calls = runner.calls();
        assert!(calls.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_rejects_symlinked_unit_path() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");

        let link = dir.path().join("systemd/user/obsctl.service");
        let real = dir.path().join("systemd/user/obsctl-real.service");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::write(&real, b"[Unit]").unwrap();
        symlink(&real, &link).unwrap();
        let symlinked = ServiceInstaller::new(&runner, link.clone());

        let err = symlinked.uninstall().unwrap_err();
        assert!(err.to_string().contains("path is symbolic link"));
        assert!(real.exists());
        assert!(link.exists());
        let calls = runner.calls();
        assert!(calls.is_empty());
    }

    #[test]
    fn uninstall_rejects_non_file_unit_path() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new("ok");
        let dir_unit = dir.path().join("systemd/user/obsctl.service");
        std::fs::create_dir_all(&dir_unit).unwrap();

        let installer = ServiceInstaller::new(&runner, dir_unit.clone());
        let err = installer.uninstall().unwrap_err();
        assert!(err.to_string().contains("path is not a regular file"));
        assert!(dir_unit.exists());
        let calls = runner.calls();
        assert!(calls.is_empty());
    }
}
