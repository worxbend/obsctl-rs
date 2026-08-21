use std::path::{Path, PathBuf};

use crate::domain::errors::ObsctlError;
use crate::domain::result::Result;
use crate::support::fs;

const SYSTEMCTL_PROGRAM_NAME: &str = "systemctl";
const SAFE_SYSTEMCTL_PATHS: [&str; 4] = [
    "/usr/bin/systemctl",
    "/bin/systemctl",
    "/usr/sbin/systemctl",
    "/sbin/systemctl",
];
pub const SYSTEMCTL_ENABLE_HINT: &str = "systemctl --user enable --now obsctl.service";
const SAFE_SYSTEMCTL_PATH_ENV: &str = "/usr/bin:/bin";
const SYSTEMCTL_RUNTIME_DIR_ENV: &str = "XDG_RUNTIME_DIR";

pub fn unit_file_path() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("systemd/user/obsctl.service"))
}

pub fn unit_file_content(exec_path: &Path) -> String {
    let exec_path = quote_exec_path(exec_path);
    format!(
        "[Unit]\n\
         Description=obsctl OBS WebSocket control daemon\n\
         After=graphical-session.target\n\
         StartLimitIntervalSec=0\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exec} server --headless\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exec = exec_path
    )
}

fn quote_exec_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw
        .replace('%', "%%")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String>;
}

pub struct SystemctlRunner;

impl CommandRunner for SystemctlRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        let program = resolve_systemctl_program(program)?;
        let output = std::process::Command::new(&program)
            .args(args)
            .env_clear()
            .env("PATH", SAFE_SYSTEMCTL_PATH_ENV)
            .envs(systemctl_safe_env_vars())
            .output()
            .map_err(|e| ObsctlError::ServiceInstallFailed(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ObsctlError::ServiceInstallFailed(format!(
                "{} {:?} failed: {stderr}",
                program.display(),
                args
            )));
        }
        Ok(stdout)
    }
}

fn invalid_systemctl_path(program: &str) -> ObsctlError {
    ObsctlError::ServiceInstallFailed(format!("invalid systemctl path: {program}"))
}

fn resolve_systemctl_program(program: &str) -> Result<PathBuf> {
    if program.is_empty() {
        return Err(ObsctlError::ServiceInstallFailed(
            "refusing to run empty program name".to_string(),
        ));
    }

    let candidate = Path::new(program);
    if candidate.is_absolute() {
        if fs::has_path_traversal(candidate) {
            return Err(invalid_systemctl_path(program));
        }
        if candidate.file_name().and_then(|name| name.to_str()) != Some(SYSTEMCTL_PROGRAM_NAME) {
            return Err(invalid_systemctl_path(program));
        }
        validate_systemctl_binary(candidate)
            .map_err(|error| ObsctlError::ServiceInstallFailed(error.to_string()))?;
        return Ok(candidate.to_path_buf());
    }
    if candidate.components().count() > 1 {
        return Err(invalid_systemctl_path(program));
    }

    if program != SYSTEMCTL_PROGRAM_NAME {
        return Err(ObsctlError::ServiceInstallFailed(format!(
            "invalid system service command: {program}"
        )));
    }

    for candidate in SAFE_SYSTEMCTL_PATHS {
        let path = Path::new(candidate);
        if validate_systemctl_binary(path).is_ok() {
            return Ok(path.to_path_buf());
        }
    }

    Err(ObsctlError::ServiceInstallFailed(
        "failed to resolve a safe systemctl binary".to_string(),
    ))
}

fn systemctl_safe_env_vars() -> Vec<(String, String)> {
    let mut envs = Vec::new();

    let xdg_runtime_dir =
        match crate::support::validation::read_env_path_token(SYSTEMCTL_RUNTIME_DIR_ENV) {
            Some(value) => value,
            None => return envs,
        };

    let path = Path::new(&xdg_runtime_dir);
    if !path.is_absolute() {
        return envs;
    }

    if fs::has_symlink(path) {
        return envs;
    }

    envs.push((SYSTEMCTL_RUNTIME_DIR_ENV.to_string(), xdg_runtime_dir));
    envs
}

/// Refuse to execute a systemctl binary that anyone but its owner could have
/// replaced.
///
/// The permission bits are read from the metadata `ensure_regular_file_with_metadata`
/// already returned, rather than by stating the path a second time. Two stats
/// leave a window in which the thing that was checked and the thing that is
/// described are not the same file.
#[cfg(unix)]
fn validate_systemctl_binary(path: &Path) -> std::io::Result<()> {
    fs::ensure_path_not_symlink(path)?;
    let metadata = fs::ensure_regular_file_with_metadata(path)?;

    use std::os::unix::fs::PermissionsExt;
    let permissions = metadata.permissions();

    if permissions.mode() & 0o111 == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "systemctl path is not executable",
        ));
    }
    if permissions.mode() & 0o022 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "systemctl path is writable by group or other",
        ));
    }
    if permissions.mode() & 0o7000 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "systemctl path has insecure special mode bits",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_systemctl_binary(path: &Path) -> std::io::Result<()> {
    fs::ensure_path_not_symlink(path)?;
    fs::ensure_regular_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn with_env_var<R>(name: &str, value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os(name);
        if let Some(value) = value {
            unsafe { std::env::set_var(name, value) };
        } else {
            unsafe { std::env::remove_var(name) };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var(name, previous) },
            None => unsafe { std::env::remove_var(name) },
        }
        result
    }

    fn with_path_env<R>(value: &str, f: impl FnOnce() -> R) -> R {
        with_env_var("PATH", Some(value), f)
    }

    #[test]
    fn unit_file_contains_exec_path() {
        let content = unit_file_content(Path::new("/usr/local/bin/obsctl"));
        assert!(content.contains("\"/usr/local/bin/obsctl\" server --headless"));
        assert!(content.contains("[Unit]"));
        assert!(content.contains("[Service]"));
        assert!(content.contains("[Install]"));
        assert!(content.contains("WantedBy=graphical-session.target"));
        assert!(!content.contains("sudo"));
    }

    #[test]
    fn unit_file_escapes_exec_path_with_spaces() {
        let content = unit_file_content(Path::new("/tmp/My App/obsctl"));
        assert!(content.contains(r#""/tmp/My App/obsctl" server --headless"#));
    }

    #[test]
    fn unit_file_escapes_exec_path_with_quotes() {
        let content = unit_file_content(Path::new("/tmp/weird\"app/obsctl"));
        assert!(content.contains(r#""/tmp/weird\"app/obsctl" server --headless"#));
    }

    #[test]
    fn unit_file_escapes_exec_path_with_percent() {
        let content = unit_file_content(Path::new("/tmp/we%ird%env/obsctl"));
        assert!(content.contains(r#""/tmp/we%%ird%%env/obsctl" server --headless"#));
    }

    #[test]
    fn unit_file_escapes_exec_path_with_dollar() {
        let content = unit_file_content(Path::new("/tmp/we$ird/path"));
        assert!(content.contains(r#""/tmp/we\$ird/path" server --headless"#));
    }

    #[test]
    fn unit_file_escapes_exec_path_with_newline() {
        let content = unit_file_content(Path::new("/tmp/weird\npath/obsctl"));
        assert!(content.contains(r#""/tmp/weird\npath/obsctl" server --headless"#));
    }

    #[test]
    fn unit_file_escapes_exec_path_with_tab() {
        let content = unit_file_content(Path::new("/tmp/weird\tpath/obsctl"));
        assert!(content.contains(r#""/tmp/weird\tpath/obsctl" server --headless"#));
    }

    #[test]
    fn resolve_systemctl_program_rejects_empty_program() {
        assert!(matches!(
            resolve_systemctl_program("").unwrap_err(),
            ObsctlError::ServiceInstallFailed(_)
        ));
    }

    #[test]
    fn resolve_systemctl_program_rejects_non_systemctl_program_name() {
        assert!(matches!(
            resolve_systemctl_program("rm"),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }

    #[test]
    fn resolve_systemctl_program_accepts_systemctl_absolute_path() {
        use std::fs::OpenOptions;
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systemctl");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(b"#!/bin/sh\necho systemctl shim\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata().unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }

        assert!(resolve_systemctl_program(path.to_str().unwrap()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_systemctl_program_rejects_relative_path_with_parent() {
        assert!(matches!(
            resolve_systemctl_program("./systemctl"),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_systemctl_program_rejects_absolute_symlink() {
        use std::fs::File;
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target-systemctl");
        let link = dir.path().join("systemctl");
        let _ = File::create(&target).unwrap();
        symlink(&target, &link).unwrap();

        let err = resolve_systemctl_program(link.to_str().unwrap());
        assert!(matches!(err, Err(ObsctlError::ServiceInstallFailed(_))));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_systemctl_program_rejects_absolute_non_systemctl_binary() {
        use std::fs::OpenOptions;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cat");
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .unwrap();

        assert!(matches!(
            resolve_systemctl_program(path.to_str().unwrap()),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_systemctl_program_rejects_non_executable_absolute_path() {
        use std::fs::OpenOptions;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systemctl");
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut perms = path.metadata().unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();

        assert!(matches!(
            resolve_systemctl_program(path.to_str().unwrap()),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_systemctl_program_rejects_insecure_systemctl_permissions() {
        use std::fs::OpenOptions;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systemctl");
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut perms = path.metadata().unwrap().permissions();
        perms.set_mode(0o775);
        std::fs::set_permissions(&path, perms).unwrap();

        assert!(matches!(
            resolve_systemctl_program(path.to_str().unwrap()),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_relative_xdg_runtime_dir() {
        use std::os::unix::fs::PermissionsExt;
        let runner = SystemctlRunner;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systemctl");
        {
            let mut script = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            script
                .write_all(b"#!/bin/sh\necho \"$XDG_RUNTIME_DIR\"\n")
                .unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        with_env_var("XDG_RUNTIME_DIR", Some("relative/runtime"), || {
            let out = runner
                .run(path.to_str().unwrap(), &[])
                .expect("systemctl runner should execute script");
            assert!(out.trim_end().is_empty());
        });
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_traversal_xdg_runtime_dir() {
        use std::os::unix::fs::PermissionsExt;
        let runner = SystemctlRunner;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("systemctl");
        {
            let mut script = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            script
                .write_all(b"#!/bin/sh\necho \"$XDG_RUNTIME_DIR\"\n")
                .unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        with_env_var("XDG_RUNTIME_DIR", Some("/tmp/../run/user"), || {
            let out = runner
                .run(path.to_str().unwrap(), &[])
                .expect("systemctl runner should execute script");
            assert!(out.trim_end().is_empty());
        });
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_symlinked_xdg_runtime_dir() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        use tempfile::tempdir;

        let runner = SystemctlRunner;
        let dir = tempdir().unwrap();
        let real_runtime_dir = dir.path().join("real-xdg");
        let linked_runtime_dir = dir.path().join("linked-xdg");
        std::fs::create_dir_all(&real_runtime_dir).unwrap();
        symlink(&real_runtime_dir, &linked_runtime_dir).unwrap();

        let path = dir.path().join("systemctl");
        {
            let mut script = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            script
                .write_all(b"#!/bin/sh\necho \"$XDG_RUNTIME_DIR\"\n")
                .unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        with_env_var(
            "XDG_RUNTIME_DIR",
            Some(linked_runtime_dir.to_str().unwrap()),
            || {
                let out = runner
                    .run(path.to_str().unwrap(), &[])
                    .expect("systemctl runner should execute script");
                assert!(out.trim_end().is_empty());
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn run_rejects_inherited_path_environment() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let runner = SystemctlRunner;
        let dir = tempdir().unwrap();
        let path = dir.path().join("systemctl");
        {
            let mut script = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            script.write_all(b"#!/bin/sh\necho \"$PATH\"\n").unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        with_path_env("/tmp/should-not-be-used:/not-a-real-dir", || {
            let out = runner
                .run(path.to_str().unwrap(), &[])
                .expect("systemctl runner should execute script");
            assert_eq!(out.trim_end(), "/usr/bin:/bin");
        });
    }

    #[cfg(unix)]
    #[test]
    fn run_preserves_safe_xdg_runtime_dir() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;

        let runner = SystemctlRunner;
        let dir = tempdir().unwrap();
        let runtime_dir = dir.path().join("xdg");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let path = dir.path().join("systemctl");
        {
            let mut script = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .unwrap();
            script
                .write_all(b"#!/bin/sh\necho \"$XDG_RUNTIME_DIR\"\n")
                .unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime_dir_display = runtime_dir.to_string_lossy().into_owned();

        with_env_var(
            "XDG_RUNTIME_DIR",
            Some(runtime_dir_display.as_str()),
            || {
                let out = runner
                    .run(path.to_str().unwrap(), &[])
                    .expect("systemctl runner should execute script");
                assert_eq!(out.trim_end(), runtime_dir_display);
            },
        );
    }

    #[test]
    fn resolve_systemctl_program_rejects_absolute_path_with_parent_component() {
        assert!(matches!(
            resolve_systemctl_program("/usr/bin/../bin/systemctl"),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }

    #[test]
    fn resolve_systemctl_program_rejects_absolute_path_with_curdir_component() {
        assert!(matches!(
            resolve_systemctl_program("/usr/./bin/systemctl"),
            Err(ObsctlError::ServiceInstallFailed(_))
        ));
    }
}
