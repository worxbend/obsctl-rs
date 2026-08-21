use std::fs::{self as stdfs, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::path::PathBuf;

/// The three refusals this module issues most often, each built in one place
/// so its kind and its wording cannot drift between call sites. Callers match
/// on `ErrorKind`, so a guard that reported the wrong kind would be silently
/// tolerated somewhere it should not be.
fn symlink_rejected() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "path is symbolic link")
}

fn no_parent() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "path has no parent")
}

fn not_regular_file() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "path is not a regular file")
}

pub fn has_symlink(path: &Path) -> bool {
    let mut cursor = Path::new("").to_path_buf();
    for component in path.components() {
        cursor.push(component);
        if let Ok(metadata) = std::fs::symlink_metadata(&cursor)
            && metadata.file_type().is_symlink()
        {
            return true;
        }
    }
    false
}

pub fn has_path_traversal(path: &Path) -> bool {
    path.as_os_str()
        .to_string_lossy()
        .split(['/', '\\'])
        .any(|component| matches!(component, "." | ".."))
}

pub fn is_private_dir_metadata(metadata: &std::fs::Metadata) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }

    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode();
        let uid = metadata.uid();
        mode & 0o002 == 0 && uid == current_uid()
    }

    #[cfg(not(unix))]
    {
        true
    }
}

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    if has_symlink(path) {
        return Err(symlink_rejected());
    }

    let metadata = std::fs::symlink_metadata(path)?;
    if !is_private_dir_metadata(&metadata) {
        #[cfg(unix)]
        let details = format!(
            " (path={}, mode={:o}, owner={}, expected_owner={})",
            path.display(),
            metadata.permissions().mode() & 0o7777,
            metadata.uid(),
            current_uid()
        );
        #[cfg(not(unix))]
        let details = format!(" (path={})", path.display());
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("path is not a private directory{details}"),
        ));
    }

    Ok(())
}

pub fn ensure_private_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(no_parent)?;

    ensure_path_not_symlink(parent)?;

    let created = !parent.exists();
    if created {
        std::fs::create_dir_all(parent)?;
        secure_permissions(parent, 0o700)?;
    }

    ensure_private_dir(parent)?;

    Ok(())
}

fn ensure_atomic_target_safe(path: &Path, parent: &Path) -> io::Result<()> {
    ensure_path_not_symlink(path)?;
    ensure_private_dir(parent)?;
    Ok(())
}

pub fn ensure_path_not_symlink(path: &Path) -> io::Result<()> {
    if has_symlink(path) {
        return Err(symlink_rejected());
    }
    Ok(())
}

pub fn ensure_regular_file(path: &Path) -> io::Result<()> {
    ensure_regular_file_with_metadata(path).map(|_| ())
}

pub fn ensure_regular_file_with_metadata(path: &Path) -> io::Result<std::fs::Metadata> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(symlink_rejected());
    }
    if !metadata.is_file() {
        return Err(not_regular_file());
    }
    Ok(metadata)
}

pub fn ensure_socket_file(path: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path exists and is not a socket: {path:?}"),
        ));
    }
    Ok(())
}

pub fn read_file_no_follow(path: &Path) -> io::Result<String> {
    ensure_path_not_symlink(path)?;

    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);

    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(not_regular_file());
    }

    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

pub fn remove_with_type_guard<F>(path: &Path, expected: F, what: &str) -> io::Result<()>
where
    F: Fn(&std::fs::Metadata) -> bool,
{
    let parent = path.parent().ok_or_else(no_parent)?;

    ensure_private_dir(parent)?;
    ensure_path_not_symlink(path)?;

    let current = match stdfs::symlink_metadata(path) {
        Ok(current) => current,
        Err(err) => {
            return Err(err);
        }
    };
    if !expected(&current) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a {what}"),
        ));
    }

    let (temp_handle, temp_path) =
        create_secure_temp_file_in(parent, "obsctl-remove-guard", 0o600)?;
    drop(temp_handle);

    let cleanup_tmp = |temp_path: &Path| {
        let _ = stdfs::remove_file(temp_path);
    };

    if let Err(err) = ensure_atomic_target_safe(path, parent) {
        cleanup_tmp(&temp_path);
        return Err(err);
    }

    if let Err(err) = stdfs::rename(path, &temp_path) {
        cleanup_tmp(&temp_path);
        return match err.kind() {
            io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        };
    }

    let metadata = match stdfs::symlink_metadata(&temp_path) {
        Ok(metadata) => metadata,
        Err(err) => {
            cleanup_tmp(&temp_path);
            return Err(err);
        }
    };

    if !expected(&metadata) {
        let restore_error = stdfs::rename(&temp_path, path).err();
        if let Some(error) = restore_error {
            cleanup_tmp(&temp_path);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path is not a {what} and could not restore original entry: {error}"),
            ));
        }

        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a {what}"),
        ));
    }

    stdfs::remove_file(&temp_path)?;
    Ok(())
}

/// Why a file is not safe for this program to execute.
///
/// The rule is one thing — "only its owner can have changed it, and it can
/// actually be run" — but it is enforced at two call sites that word their
/// refusals differently (one installs a service unit, the other picks a
/// systemctl binary). Naming the three failures here keeps the policy in one
/// place while leaving each caller its own message.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsecureExecutable {
    /// No execute bit set for anyone.
    NotExecutable,
    /// Writable by group or other, so someone else could swap its contents.
    GroupOrOtherWritable,
    /// setuid, setgid, or sticky — nothing this program runs should carry these.
    SpecialModeBits,
}

/// Check a file's permission bits against the rule above.
///
/// Takes metadata rather than a path on purpose: the caller has already
/// established what the path is, and re-stating it here would leave a window
/// in which the file checked and the file described are not the same one.
#[cfg(unix)]
pub fn check_executable_permissions(
    metadata: &std::fs::Metadata,
) -> Result<(), InsecureExecutable> {
    use std::os::unix::fs::PermissionsExt;
    let mode = metadata.permissions().mode();

    if mode & 0o111 == 0 {
        return Err(InsecureExecutable::NotExecutable);
    }
    if mode & 0o022 != 0 {
        return Err(InsecureExecutable::GroupOrOtherWritable);
    }
    if mode & 0o7000 != 0 {
        return Err(InsecureExecutable::SpecialModeBits);
    }
    Ok(())
}

pub fn secure_permissions(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        stdfs::set_permissions(path, stdfs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        std::fs::metadata("/proc/self")
            .or_else(|_| std::fs::metadata("."))
            .map(|m| {
                use std::os::unix::fs::MetadataExt as _;
                m.uid()
            })
            .unwrap_or(0)
    }

    #[cfg(not(unix))]
    {
        0
    }
}

pub fn create_secure_temp_file_in(
    dir: &Path,
    name_prefix: &str,
    mode: u32,
) -> io::Result<(File, PathBuf)> {
    for _ in 0..1024 {
        let suffix: u64 = rand::random();
        let tmp_path = dir.join(format!(".{name_prefix}.{suffix:016x}.tmp"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }

        match options.open(&tmp_path) {
            Ok(file) => {
                #[cfg(unix)]
                return Ok((file, tmp_path));

                #[cfg(not(unix))]
                {
                    if let Err(err) = secure_permissions(&tmp_path, mode) {
                        let _ = stdfs::remove_file(&tmp_path);
                        return Err(err);
                    }
                    return Ok((file, tmp_path));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary file name",
    ))
}

pub fn write_atomic_with_temp_file<F>(
    path: &Path,
    tmp_prefix: &str,
    mode: u32,
    allow_overwrite: bool,
    mut write: F,
) -> io::Result<()>
where
    F: FnMut(&mut File) -> io::Result<()>,
{
    let cleanup_tmp = |tmp_path: &Path| {
        let _ = stdfs::remove_file(tmp_path);
    };

    let parent = path.parent().ok_or_else(no_parent)?;

    ensure_private_parent(path)?;

    let (mut tmp, tmp_path) = create_secure_temp_file_in(parent, tmp_prefix, mode)?;

    if let Err(err) = write(&mut tmp) {
        drop(tmp);
        let _ = stdfs::remove_file(&tmp_path);
        return Err(err);
    }
    tmp.flush()?;
    tmp.sync_all()?;
    drop(tmp);

    // Re-check destination safety immediately before the rename.
    if let Err(err) = ensure_atomic_target_safe(path, parent) {
        cleanup_tmp(&tmp_path);
        return Err(err);
    }

    match stdfs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => {
            cleanup_tmp(&tmp_path);
            return Err(not_regular_file());
        }
        Ok(_) if !allow_overwrite => {
            cleanup_tmp(&tmp_path);
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            cleanup_tmp(&tmp_path);
            return Err(error);
        }
    }

    if let Err(error) = stdfs::rename(&tmp_path, path) {
        cleanup_tmp(&tmp_path);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn has_path_traversal_detects_dot_components() {
        assert!(has_path_traversal(Path::new("/tmp/./foo")));
        assert!(has_path_traversal(Path::new("/tmp/../foo")));
        assert!(!has_path_traversal(Path::new("/tmp/foo/bar")));
        assert!(!has_path_traversal(Path::new("/")));
    }

    #[test]
    fn write_atomic_with_temp_file_writes_file_atomically() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yml");
        let prefix = "obsctl-test";

        write_atomic_with_temp_file(&path, prefix, 0o600, true, |tmp| {
            use std::io::Write as _;
            tmp.write_all(b"hello")
        })
        .unwrap();

        let value = std::fs::read_to_string(&path).unwrap();
        assert_eq!(value, "hello");
        let metadata = std::fs::metadata(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn write_atomic_with_temp_file_rejects_overwrite_when_not_allowed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "existing").unwrap();

        let err = write_atomic_with_temp_file(&path, "obsctl-test", 0o600, false, |tmp| {
            use std::io::Write as _;
            tmp.write_all(b"updated")
        })
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_with_temp_file_refuses_overwrite_of_non_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("not-a-file");
        let _ = std::fs::create_dir(&path);
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));

        let err = write_atomic_with_temp_file(&path, "obsctl-test", 0o600, true, |tmp| {
            use std::io::Write as _;
            tmp.write_all(b"updated")
        })
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_with_temp_file_refuses_unsafe_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let parent = dir.path().join("unsafe");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = parent.join("config.yml");

        let err = write_atomic_with_temp_file(&path, "obsctl-test", 0o600, true, |tmp| {
            use std::io::Write as _;
            tmp.write_all(b"updated")
        })
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!path.exists());

        let mut tmp_count = 0usize;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".obsctl-test.") && name.ends_with(".tmp") {
                tmp_count += 1;
            }
        }
        assert_eq!(tmp_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn create_secure_temp_file_in_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let (tmp, path) = create_secure_temp_file_in(dir.path(), "obsctl-perm", 0o600).unwrap();
        let mode = tmp.metadata().unwrap().permissions().mode();
        drop(tmp);
        assert_eq!(mode & 0o777, 0o600);
        assert!(path.exists());
        let _ = std::fs::remove_file(&path);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn remove_with_type_guard_rejects_symlink_path() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&real, "payload").unwrap();
        symlink(&real, &link).unwrap();

        let result = remove_with_type_guard(
            &link,
            |metadata| metadata.file_type().is_file(),
            "regular file",
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(link.exists());
        assert!(real.exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_with_temp_file_rejects_overwrite_of_symlink_destination() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let target = dir.path().join("real-target");
        let link = dir.path().join("link.yml");
        std::fs::write(&target, "version: 1\n").unwrap();
        symlink(&target, &link).unwrap();

        let err = write_atomic_with_temp_file(&link, "obsctl-unsafe-link", 0o600, true, |tmp| {
            use std::io::Write as _;
            tmp.write_all(b"updated")
        })
        .unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let mut tmp_count = 0usize;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".obsctl-unsafe-link.") && name.ends_with(".tmp") {
                tmp_count += 1;
            }
        }
        assert_eq!(tmp_count, 0);
    }

    #[test]
    fn write_atomic_with_temp_file_cleans_temp_on_write_failure() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yml");
        let _ = write_atomic_with_temp_file(&path, "obsctl-temp-cleanup", 0o600, true, |_tmp| {
            Err(std::io::Error::other("boom"))
        })
        .err();

        let mut tmp_count = 0usize;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".obsctl-temp-cleanup.") && name.ends_with(".tmp") {
                tmp_count += 1;
            }
        }
        assert_eq!(tmp_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_regular_file_rejects_symlink_and_non_file_paths() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let file = dir.path().join("unit");
        std::fs::write(&file, "data").unwrap();
        assert!(ensure_regular_file(&file).is_ok());

        let real_dir = dir.path().join("dir");
        std::fs::create_dir(&real_dir).unwrap();
        assert_eq!(
            ensure_regular_file(&real_dir).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );

        let link = dir.path().join("link");
        symlink(&file, &link).unwrap();
        assert_eq!(
            ensure_regular_file(&link).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_regular_file_with_metadata_returns_metadata() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let file = dir.path().join("unit");
        std::fs::write(&file, "data").unwrap();
        let metadata = ensure_regular_file_with_metadata(&file).unwrap();
        assert!(metadata.is_file());

        let link = dir.path().join("link");
        symlink(&file, &link).unwrap();
        assert_eq!(
            ensure_regular_file_with_metadata(&link).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );

        let err = ensure_regular_file_with_metadata(dir.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_socket_file_rejects_invalid_paths() {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let dir = tempdir().unwrap();

        let socket_path = dir.path().join("socket");
        let listener = UnixListener::bind(&socket_path).unwrap();
        assert!(ensure_socket_file(&socket_path).is_ok());
        drop(listener);

        let regular_path = dir.path().join("regular");
        std::fs::write(&regular_path, "payload").unwrap();
        assert_eq!(
            ensure_socket_file(&regular_path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );

        let link = dir.path().join("socket-link");
        symlink(&socket_path, &link).unwrap();
        assert_eq!(
            ensure_socket_file(&link).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_with_temp_file_rejects_symlink_paths() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let real = dir.path().join("real.yml");
        let link = dir.path().join("link.yml");
        std::fs::write(&real, "real").unwrap();
        symlink(&real, &link).unwrap();

        assert!(
            write_atomic_with_temp_file(&link, "obsctl-test", 0o600, true, |tmp| {
                use std::io::Write as _;
                tmp.write_all(b"evil")
            })
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_file_no_follow_rejects_symlink_path() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        let link = dir.path().join("link");
        std::fs::write(&real, "ok").unwrap();
        symlink(&real, &link).unwrap();

        assert!(matches!(
            read_file_no_follow(&link).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
                | std::io::ErrorKind::PermissionDenied
                | std::io::ErrorKind::Other
        ));
    }

    #[test]
    fn read_file_no_follow_rejects_non_file_path() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let err = read_file_no_follow(path).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn remove_with_type_guard_refuses_non_matching_type() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("regular.txt");
        std::fs::write(&path, "payload").unwrap();

        let err =
            remove_with_type_guard(&path, |metadata| metadata.file_type().is_socket(), "socket")
                .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(path.exists());

        let mut tmp_count = 0usize;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".obsctl-remove-guard.") && name.ends_with(".tmp") {
                tmp_count += 1;
            }
        }
        assert_eq!(tmp_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_parent_rejects_symlinked_ancestor_without_creating_path() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let real_parent = dir.path().join("real-parent");
        let link_parent = dir.path().join("link-parent");
        let nested_target = link_parent.join("child");

        std::fs::create_dir(&real_parent).unwrap();
        symlink(&real_parent, &link_parent).unwrap();

        let err = ensure_private_parent(&nested_target);
        assert_eq!(
            err.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(!nested_target.exists());
        assert!(std::fs::metadata(&real_parent).is_ok());
    }
}
