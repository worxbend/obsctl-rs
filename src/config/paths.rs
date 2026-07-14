use crate::support::validation::read_env_path_token;
use std::path::{Path, PathBuf};

pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "obsctl").map(|d| d.config_dir().join("config.yml"))
}

pub fn config_path() -> Option<PathBuf> {
    if let Some(trimmed) = read_env_path_token("OBSCTL_CONFIG") {
        let candidate = PathBuf::from(&trimmed);
        if !candidate.is_absolute() {
            return default_config_path();
        }
        if crate::support::fs::has_symlink(candidate.as_path()) {
            return default_config_path();
        }
        return Some(candidate);
    }
    default_config_path()
}

/// Directory that holds locale override files (`en.yml`, `uk.yml`, ...),
/// sitting alongside whichever config file is in effect. Falls back to the
/// default config directory when `config_path` is `None` (e.g. no config
/// file could be resolved at all).
pub fn locales_dir(config_path: Option<&Path>) -> Option<PathBuf> {
    let base = match config_path {
        Some(p) => p.parent().map(Path::to_path_buf),
        None => default_config_path().and_then(|p| p.parent().map(Path::to_path_buf)),
    }?;
    Some(base.join("locales"))
}

pub fn default_log_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "obsctl").map(|d| {
        d.state_dir()
            .unwrap_or(d.data_local_dir())
            .join("obsctl.log")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    fn with_obsctl_config_env<R>(value: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os("OBSCTL_CONFIG");
        if let Some(value) = value {
            unsafe { std::env::set_var("OBSCTL_CONFIG", value) };
        } else {
            unsafe { std::env::remove_var("OBSCTL_CONFIG") };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var("OBSCTL_CONFIG", previous) },
            None => unsafe { std::env::remove_var("OBSCTL_CONFIG") },
        }

        result
    }

    #[cfg(unix)]
    fn with_obsctl_config_env_os<R>(value: Option<std::ffi::OsString>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = std::env::var_os("OBSCTL_CONFIG");
        if let Some(value) = value {
            unsafe { std::env::set_var("OBSCTL_CONFIG", value) };
        } else {
            unsafe { std::env::remove_var("OBSCTL_CONFIG") };
        }

        let result = f();

        match previous {
            Some(previous) => unsafe { std::env::set_var("OBSCTL_CONFIG", previous) },
            None => unsafe { std::env::remove_var("OBSCTL_CONFIG") },
        }

        result
    }

    #[test]
    fn config_path_uses_trimmed_env_override() {
        with_obsctl_config_env(Some("  /tmp/obsctl.yml  "), || {
            assert_eq!(config_path(), Some(PathBuf::from("/tmp/obsctl.yml")));
        });
    }

    #[test]
    fn locales_dir_sits_beside_given_config_path() {
        let dir = locales_dir(Some(Path::new("/tmp/obsctl-test/config.yml"))).unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/obsctl-test/locales"));
    }

    #[test]
    fn locales_dir_falls_back_to_default_config_dir() {
        let expected = default_config_path()
            .unwrap()
            .parent()
            .unwrap()
            .join("locales");
        assert_eq!(locales_dir(None), Some(expected));
    }

    #[test]
    fn config_path_empty_env_falls_back_to_default() {
        let fallback = default_config_path();
        with_obsctl_config_env(Some("   "), || {
            assert_eq!(config_path(), fallback);
        });
    }

    #[test]
    fn config_path_rejects_control_characters() {
        let fallback = default_config_path();
        with_obsctl_config_env(Some("/tmp/\nobsctl.yml"), || {
            assert_eq!(config_path(), fallback);
        });
    }

    #[test]
    fn config_path_rejects_relative_override() {
        let fallback = default_config_path();
        with_obsctl_config_env(Some("relative/config.yml"), || {
            assert_eq!(config_path(), fallback);
        });
    }

    #[test]
    fn config_path_rejects_traversal_override() {
        let fallback = default_config_path();
        with_obsctl_config_env(Some("/tmp/../obsctl.yml"), || {
            assert_eq!(config_path(), fallback);
        });
    }

    #[test]
    fn config_path_rejects_excessive_length() {
        let fallback = default_config_path();
        let value = "a".repeat(crate::support::validation::MAX_PATH_TOKEN_LENGTH + 1);
        with_obsctl_config_env(Some(&value), || {
            assert_eq!(config_path(), fallback);
        });
    }

    #[cfg(unix)]
    #[test]
    fn config_path_rejects_symlinked_override() {
        use std::os::unix::fs::symlink;

        let fallback = default_config_path();
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let link_parent = dir.path().join("linked");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &link_parent).unwrap();

        let linked_path = link_parent.join("config.yml");
        with_obsctl_config_env(Some(linked_path.to_string_lossy().as_ref()), || {
            assert_eq!(config_path(), fallback);
        });
    }

    #[cfg(unix)]
    #[test]
    fn config_path_rejects_non_unicode_override() {
        let fallback = default_config_path();
        let value = {
            let mut bytes = b"/tmp/".to_vec();
            bytes.push(0xff);
            bytes.extend_from_slice(b"obsctl.yml");
            std::ffi::OsString::from_vec(bytes)
        };
        with_obsctl_config_env_os(Some(value), || {
            assert_eq!(config_path(), fallback);
        });
    }
}
