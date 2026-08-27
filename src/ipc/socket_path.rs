use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt as _;

use crate::support;
use crate::support::validation::read_env_path_token;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketPathValidationError {
    Token(crate::support::validation::ValidationError),
    LeadingTrailingWhitespace,
    UnsafePath(String),
}

impl std::fmt::Display for SocketPathValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(error) => write!(f, "must be a valid path token ({error})"),
            Self::LeadingTrailingWhitespace => {
                write!(f, "must not have leading or trailing whitespace")
            }
            Self::UnsafePath(error) => {
                write!(
                    f,
                    "socket path must be an absolute, non-symlink, private filesystem path: {error}"
                )
            }
        }
    }
}

pub fn default_socket_path() -> PathBuf {
    if let Some(runtime_dir) = read_env_path_token("XDG_RUNTIME_DIR") {
        let runtime_dir = Path::new(&runtime_dir);
        if is_valid_runtime_dir(runtime_dir) {
            return runtime_dir.join("obsctl").join("obsctl.sock");
        }
    }
    let uid = support::fs::current_uid();
    PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"))
}

/// A relative socket path is refused rather than resolved, because what it
/// would resolve against — the process's working directory — is not something
/// the daemon controls or the config author can see.
fn require_absolute(path: &Path) -> io::Result<()> {
    if path.is_absolute() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "socket path must be absolute",
    ))
}

pub fn ensure_private_socket_parent(path: &Path) -> io::Result<()> {
    require_absolute(path)?;
    support::fs::ensure_private_parent(path)
}

pub fn validate_socket_path(path: &Path) -> io::Result<()> {
    require_absolute(path)?;

    if support::fs::has_path_traversal(path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path contains traversal components",
        ));
    }

    if path.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path must include a filename",
        ));
    }

    if support::fs::has_symlink(path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket path is symbolic link",
        ));
    }

    if let Some(parent) = path.parent() {
        if parent.exists() {
            support::fs::ensure_private_dir(parent)?;
        }
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path has no parent",
        ))
    }
}

pub fn ensure_socket_file(path: &Path) -> io::Result<()> {
    crate::support::fs::ensure_socket_file(path)
}

pub fn parse_server_socket_path(
    raw_socket_path: &str,
) -> Result<PathBuf, SocketPathValidationError> {
    let socket_path = crate::support::validation::trim_and_validate_path_token(raw_socket_path)
        .map_err(SocketPathValidationError::Token)?;

    if socket_path != raw_socket_path {
        return Err(SocketPathValidationError::LeadingTrailingWhitespace);
    }

    let socket_path = PathBuf::from(socket_path);
    if let Err(error) = validate_socket_path(&socket_path) {
        return Err(SocketPathValidationError::UnsafePath(error.to_string()));
    }

    Ok(socket_path)
}

pub fn parse_optional_server_socket_path(
    raw_socket_path: Option<&str>,
) -> Result<Option<PathBuf>, SocketPathValidationError> {
    raw_socket_path.map(parse_server_socket_path).transpose()
}

pub fn resolve_server_socket_path(
    raw_socket_path: Option<&str>,
) -> Result<PathBuf, SocketPathValidationError> {
    parse_optional_server_socket_path(raw_socket_path)
        .map(|path| path.unwrap_or_else(default_socket_path))
}

/// Get `socket_path` ready for a daemon to bind: a private parent directory
/// exists, the path itself is one this process is willing to use, and nothing
/// is left over from a previous run.
///
/// The error is the message to log, already written out. That is unusual, and
/// it is deliberate: the three ways this can fail are three different pieces of
/// socket-file policy, and the wording that explains each of them belongs
/// beside the rule it explains rather than in the daemon's startup code, where
/// it used to sit two hundred lines away from anything that could change it.
pub async fn prepare(socket_path: &Path) -> Result<(), String> {
    if let Err(error) = ensure_private_socket_parent(socket_path) {
        return Err(format!(
            "Refusing to use socket path {}: {error}",
            socket_path.display()
        ));
    }

    if let Err(error) = validate_socket_path(socket_path) {
        return Err(format!(
            "Invalid socket path {}: {error}",
            socket_path.display()
        ));
    }

    // A socket file left behind by a crashed daemon must be cleared, but one
    // belonging to a daemon that is still running must not be.
    if socket_path.exists()
        && let Err(error) = try_remove_stale_socket(socket_path).await
    {
        return Err(format!("Socket path occupied by live server: {error}"));
    }

    Ok(())
}

/// Remove the socket file a daemon is finished with.
///
/// Distinct from [`prepare`]'s stale-socket handling in one respect only: this
/// runs on the way out, when the socket being ours is not in question, so it
/// does not probe for a live server first. Everything after that decision is
/// shared — see [`remove_socket_file_if_stale`].
pub fn cleanup(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) => support::fs::ensure_private_dir(parent)?,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "socket path has no parent",
            ));
        }
    }

    remove_socket_file_if_stale(path)
}

/// Whether the directory that would hold `path` is one only this user can
/// write to.
///
/// A typed answer to a question the daemon used to ask by searching an
/// `io::Error`'s message for the words "not a private directory". That wording
/// belongs to `support::fs::ensure_private_dir`, two modules away, and no test
/// pinned it, so rewording it would silently have changed whether the daemon
/// was willing to delete a socket file sitting in a world-writable directory.
///
/// A path with no parent at all (`/`) counts as not private: there is no
/// directory to judge, and refusing is the safe answer.
fn socket_parent_is_private(path: &Path) -> bool {
    path.parent()
        .is_some_and(|parent| support::fs::ensure_private_dir(parent).is_ok())
}

/// Clear a socket file left behind by a daemon that is no longer running.
///
/// The liveness probe is the whole point: a socket file on disk says nothing
/// about whether anyone is listening on it, and the two cases need opposite
/// treatment. Connecting is the only way to tell them apart — a connection that
/// is accepted means a live server owns this path and this process must not
/// start; a refusal or a timeout means the file outlived its daemon and is safe
/// to unlink.
async fn try_remove_stale_socket(path: &Path) -> Result<(), String> {
    // Checked first, and separately from the rest, so that a relative path is
    // told it is relative rather than being asked about a parent directory
    // that means nothing without a working directory to resolve against.
    require_absolute(path).map_err(|error| error.to_string())?;

    if !socket_parent_is_private(path) {
        return Err("socket parent is not private".to_string());
    }

    validate_socket_path(path).map_err(|error| error.to_string())?;
    validate_socket_file_for_cleanup(path).map_err(|error| error.to_string())?;

    const LIVENESS_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

    match tokio::time::timeout(
        LIVENESS_PROBE_TIMEOUT,
        tokio::net::UnixStream::connect(path),
    )
    .await
    {
        Ok(Ok(_)) => Err(format!(
            "live server is already running at {}",
            path.display()
        )),
        _ => remove_socket_file_if_stale(path).map_err(|error| error.to_string()),
    }
}

/// Check one more time that `path` is a socket file worth touching, then
/// unlink it.
///
/// The two steps are one function because of the gap between them. However
/// recently a caller validated this path, another process can replace it in the
/// meantime, so the check has to sit as close to the delete as it can — and
/// `remove_with_type_guard` re-reads the metadata a third time for the same
/// reason. A file that has vanished at either step is success, not an error:
/// both callers wanted it gone, and something else getting there first counts.
fn remove_socket_file_if_stale(path: &Path) -> io::Result<()> {
    match validate_socket_file_for_cleanup(path) {
        Ok(()) => {}
        Err(SocketFileValidation::File(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(SocketFileValidation::Path(error)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid socket path for cleanup: {error}"),
            ));
        }
        Err(SocketFileValidation::File(error)) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid socket file for cleanup: {error}"),
            ));
        }
    }

    match support::fs::remove_with_type_guard(
        path,
        |metadata| metadata.file_type().is_socket(),
        "socket",
    ) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Why a path that should hold a live IPC socket cannot be treated as one.
///
/// The two cases are kept apart for [`remove_socket_file_if_stale`], which
/// tolerates a `File` error meaning "already gone" but not a `Path` error
/// meaning "this is not a path we are willing to touch". Callers that only need
/// to report the problem use the `Display` impl.
enum SocketFileValidation {
    Path(io::Error),
    File(io::Error),
}

impl std::fmt::Display for SocketFileValidation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(error) | Self::File(error) => error.fmt(f),
        }
    }
}

fn validate_socket_file_for_cleanup(path: &Path) -> Result<(), SocketFileValidation> {
    validate_socket_path(path).map_err(SocketFileValidation::Path)?;
    ensure_socket_file(path).map_err(SocketFileValidation::File)?;
    Ok(())
}

fn is_valid_runtime_dir(path: &Path) -> bool {
    if !path.is_absolute() || crate::support::fs::has_path_traversal(path) {
        return false;
    }

    support::fs::ensure_private_dir(path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_uid() -> u32 {
        crate::support::fs::current_uid()
    }

    use std::env;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn with_xdg_runtime_dir<R>(value: Option<&Path>, f: impl FnOnce() -> R) -> R {
        with_xdg_runtime_dir_os(value.map(|path| path.as_os_str().to_os_string()), f)
    }

    fn with_xdg_runtime_dir_os<R>(value: Option<std::ffi::OsString>, f: impl FnOnce() -> R) -> R {
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let previous = env::var_os("XDG_RUNTIME_DIR");
        if let Some(value) = value {
            unsafe { env::set_var("XDG_RUNTIME_DIR", value) };
        } else {
            unsafe { env::remove_var("XDG_RUNTIME_DIR") };
        }

        let result = f();

        match previous {
            Some(prev) => unsafe { env::set_var("XDG_RUNTIME_DIR", prev) },
            None => unsafe { env::remove_var("XDG_RUNTIME_DIR") },
        }
        result
    }

    #[cfg(unix)]
    #[test]
    fn default_socket_path_uses_valid_runtime_dir() {
        let runtime = TempDir::new().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        let expected = runtime.path().join("obsctl").join("obsctl.sock");
        with_xdg_runtime_dir(Some(runtime.path()), || {
            let path = super::default_socket_path();
            assert_eq!(path, expected);
        });
    }

    #[cfg(unix)]
    #[test]
    fn default_socket_path_falls_back_when_runtime_dir_is_unsafe() {
        let runtime = TempDir::new().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o777)).unwrap();

        let uid = current_uid();
        let fallback = PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"));
        with_xdg_runtime_dir(Some(runtime.path()), || {
            let path = super::default_socket_path();
            assert_eq!(path, fallback);
        });
    }

    #[test]
    fn default_socket_path_falls_back_when_runtime_dir_is_empty_or_missing() {
        let uid = current_uid();
        let fallback = PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"));

        with_xdg_runtime_dir(None, || {
            let path = super::default_socket_path();
            assert_eq!(path, fallback);
        });

        with_xdg_runtime_dir(Some(Path::new("")), || {
            let path = super::default_socket_path();
            assert_eq!(path, fallback);
        });
    }

    #[test]
    fn default_socket_path_rejects_excessive_runtime_dir_env() {
        let uid = current_uid();
        let fallback = PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"));
        let value = format!(
            "/{}",
            "a".repeat(crate::support::validation::MAX_PATH_TOKEN_LENGTH)
        );

        with_xdg_runtime_dir(Some(Path::new(&value)), || {
            let path = super::default_socket_path();
            assert_eq!(path, fallback);
        });
    }

    #[test]
    fn default_socket_path_rejects_traversal_runtime_dir() {
        let uid = current_uid();
        let fallback = PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"));

        with_xdg_runtime_dir(Some(Path::new("/tmp/../tmp/obsctl")), || {
            let path = super::default_socket_path();
            assert_eq!(path, fallback);
        });
    }

    #[cfg(unix)]
    #[test]
    fn default_socket_path_rejects_non_unicode_runtime_dir() {
        let uid = current_uid();
        let fallback = PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"));
        let value = {
            use std::os::unix::ffi::OsStringExt;

            let mut bytes = b"/tmp/".to_vec();
            bytes.push(0xff);
            bytes.extend_from_slice(b"obs");
            std::ffi::OsString::from_vec(bytes)
        };

        with_xdg_runtime_dir_os(Some(value), || {
            let path = super::default_socket_path();
            assert_eq!(path, fallback);
        });
    }

    #[cfg(unix)]
    #[test]
    fn rejects_world_writable_socket_parent() {
        let dir = TempDir::new().unwrap();
        let parent = dir.path().join("parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(super::validate_socket_path(&parent.join("obsctl.sock")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn accepts_writable_private_runtime_dir() {
        let dir = TempDir::new().unwrap();
        let mode = 0o700;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(super::is_valid_runtime_dir(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_socket_parent_creates_private_directory() {
        use std::os::unix::fs::PermissionsExt;

        let uid = current_uid();
        let dir = PathBuf::from(format!("/tmp/obsctl-{uid}-parent-test"));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        let socket_path = dir.join("obsctl.sock");

        super::ensure_private_socket_parent(&socket_path).unwrap();
        let metadata = std::fs::symlink_metadata(&dir).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_socket_parent_rejects_missing_unsafe_ancestor_symlink() {
        use std::os::unix::fs::symlink;

        let uid = current_uid();
        let base = PathBuf::from(format!("/tmp/obsctl-{uid}-unsafe-ancestor-test"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base).unwrap();
        let real_base = base.join("real");
        let linked_parent = base.join("linked-parent");
        std::fs::create_dir(&real_base).unwrap();
        symlink(&real_base, &linked_parent).unwrap();
        let socket_path = base
            .join("linked-parent")
            .join("does-not-exist")
            .join("obsctl.sock");

        let result = super::ensure_private_socket_parent(&socket_path);
        assert!(result.is_err());

        if linked_parent.exists() {
            std::fs::remove_file(&linked_parent).ok();
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn rejects_relative_socket_path() {
        assert!(super::validate_socket_path(Path::new("relative.sock")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_runtime_dir_symbolic_links() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let link = dir.path().join("runtime-link");
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, &link).unwrap();
        assert!(!super::is_valid_runtime_dir(&link));
    }

    #[test]
    fn rejects_non_absolute_runtime_dir() {
        assert!(!super::is_valid_runtime_dir(Path::new("relative/path")));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_world_writable_runtime_dir() {
        let dir = TempDir::new().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(!super::is_valid_runtime_dir(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_socket_path_with_symlink_parent() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let real_parent = dir.path().join("real-parent");
        std::fs::create_dir(&real_parent).unwrap();

        let link_parent = dir.path().join("link-parent");
        symlink(&real_parent, &link_parent).unwrap();
        let socket_path = link_parent.join("obsctl.sock");

        assert!(super::validate_socket_path(&socket_path).is_err());
    }

    #[test]
    fn rejects_file_as_runtime_dir() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(!super::is_valid_runtime_dir(file.path()));
    }

    #[test]
    fn parse_optional_server_socket_path_returns_none_for_absent_value() {
        assert_eq!(super::parse_optional_server_socket_path(None), Ok(None));
    }

    #[test]
    fn resolve_server_socket_path_returns_default_for_absent_value() {
        // Both calls read `XDG_RUNTIME_DIR`, and sibling tests in this module
        // set and unset it around their own bodies. Without the lock the two
        // reads can straddle one of those changes and disagree — which is a
        // flake in the test, not a fault in the code under test.
        let _lock = crate::support::validation::test_env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let resolved = super::resolve_server_socket_path(None).unwrap();
        assert_eq!(resolved, super::default_socket_path());
    }

    #[test]
    fn parse_optional_server_socket_path_parses_present_value() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("obsctl.sock");
        let parsed = super::parse_optional_server_socket_path(path.to_str()).unwrap();
        assert_eq!(parsed, Some(path));
    }

    #[test]
    fn parse_optional_server_socket_path_rejects_invalid_present_value() {
        assert!(super::parse_optional_server_socket_path(Some("relative.sock")).is_err());
    }

    #[test]
    fn resolve_server_socket_path_rejects_invalid_present_value() {
        let err = super::resolve_server_socket_path(Some("relative.sock")).unwrap_err();
        assert!(
            err.to_string()
                .contains("socket path must be an absolute, non-symlink, private filesystem path")
        );
    }

    #[test]
    fn parse_server_socket_path_rejects_invalid_tokens() {
        let err = super::parse_server_socket_path("   ");
        assert!(matches!(
            err,
            Err(SocketPathValidationError::Token(
                crate::support::validation::ValidationError::Blank
            ))
        ));
    }

    #[test]
    fn parse_server_socket_path_rejects_control_characters() {
        assert!(matches!(
            super::parse_server_socket_path("/tmp/obsctl\t.sock"),
            Err(SocketPathValidationError::Token(
                crate::support::validation::ValidationError::ControlCharacters
            ))
        ));
    }

    #[test]
    fn parse_server_socket_path_rejects_wrapped_whitespace() {
        assert_eq!(
            super::parse_server_socket_path(" /tmp/obsctl.sock "),
            Err(SocketPathValidationError::LeadingTrailingWhitespace)
        );
    }

    #[test]
    fn parse_server_socket_path_rejects_excessive_length() {
        let path = format!(
            "/{}",
            "a".repeat(crate::support::validation::MAX_PATH_TOKEN_LENGTH)
        );
        let err = super::parse_server_socket_path(&path).unwrap_err();
        assert!(matches!(
            err,
            SocketPathValidationError::Token(
                crate::support::validation::ValidationError::MaxLengthExceeded(
                    crate::support::validation::MAX_PATH_TOKEN_LENGTH
                )
            )
        ));
    }

    #[test]
    fn parse_server_socket_path_rejects_relative_paths_with_reason() {
        assert_eq!(
            super::parse_server_socket_path("relative.sock"),
            Err(SocketPathValidationError::UnsafePath(
                "socket path must be absolute".to_string()
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_socket_file_rejects_non_socket_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-socket.txt");
        std::fs::write(&path, b"payload").unwrap();

        let error = super::ensure_socket_file(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("not a socket"));
    }

    #[cfg(unix)]
    #[test]
    fn ensure_socket_file_rejects_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.sock");

        let error = super::ensure_socket_file(&missing).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn validate_socket_path_rejects_relative_paths() {
        let err = super::validate_socket_path(Path::new("relative.sock")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "socket path must be absolute");
    }

    #[test]
    fn validate_socket_path_rejects_traversal_components() {
        let err = super::validate_socket_path(Path::new("/tmp/../obsctl.sock")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "socket path contains traversal components");
    }

    #[test]
    fn parse_server_socket_path_rejects_traversal_components() {
        let err = super::parse_server_socket_path("/tmp/../obsctl.sock");
        assert!(matches!(err, Err(SocketPathValidationError::UnsafePath(_))));
        assert_eq!(
            err.unwrap_err().to_string(),
            "socket path must be an absolute, non-symlink, private filesystem path: socket path contains traversal components"
        );
    }

    // ---- Socket file lifecycle ----
    //
    // These moved here from `server::daemon` along with the code they cover.
    // They are tests of socket-file policy — what this daemon may delete, and
    // when — not of the daemon's own lifecycle, and policy and its tests read
    // better in one place.

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_check_rejects_symlink_paths() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.sock");
        let link = dir.path().join("link.sock");
        symlink(&real, &link).unwrap();

        let result = super::try_remove_stale_socket(&link).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "socket path is symbolic link");
    }

    #[tokio::test]
    async fn stale_socket_check_rejects_non_socket_files() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        let path = file.path().to_path_buf();
        let mut out = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        out.write_all(b"not a socket").unwrap();

        let result = super::try_remove_stale_socket(&path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a socket"));
        assert!(path.exists());
    }

    #[tokio::test]
    async fn stale_socket_check_rejects_relative_paths() {
        let path = PathBuf::from("relative.sock");
        let result = super::try_remove_stale_socket(&path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("absolute"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_check_removes_stale_listener_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.sock");

        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        drop(listener);

        let result = super::try_remove_stale_socket(&path).await;
        assert!(result.is_ok());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stale_socket_check_rejects_unsafe_socket_parent() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("unsafe-parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = parent.join("obsctl.sock");
        std::fs::write(&path, b"payload").unwrap();

        let error = super::try_remove_stale_socket(&path).await.unwrap_err();
        assert!(error.contains("socket parent is not private"));
        assert!(path.exists());
    }

    #[test]
    fn cleanup_rejects_non_socket_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-socket.txt");
        std::fs::write(&path, b"payload").unwrap();

        let error = super::cleanup(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("not a socket"));
        assert!(path.exists());
    }

    #[test]
    fn cleanup_noop_when_path_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.sock");

        super::cleanup(&path).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_broken_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link.sock");
        let target = dir.path().join("missing.sock");
        symlink(&target, &path).unwrap();

        let error = super::cleanup(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("invalid socket path for cleanup"));
        assert!(std::fs::symlink_metadata(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_rejects_unsafe_socket_parent() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("unsafe-parent");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = parent.join("obsctl.sock");
        std::fs::write(&path, b"payload").unwrap();

        let error = super::cleanup(&path).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("not a private directory"));
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cleanup_removes_stale_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.sock");

        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        drop(listener);

        super::cleanup(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn validate_socket_path_rejects_paths_without_filename() {
        let err = super::validate_socket_path(Path::new("/")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "socket path must include a filename");
    }
}
