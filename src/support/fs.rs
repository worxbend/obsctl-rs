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

    let current = stdfs::symlink_metadata(path)?;
    if !expected(&current) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a {what}"),
        ));
    }

    let (temp_handle, temp) = create_secure_temp_file_in(parent, "obsctl-remove-guard", 0o600)?;
    drop(temp_handle);
    // From here on every `return` leaves `temp` behind, and dropping it removes
    // the file. Nothing below has to remember to clean up.

    ensure_atomic_target_safe(path, parent)?;

    if let Err(err) = stdfs::rename(path, temp.path()) {
        return match err.kind() {
            io::ErrorKind::NotFound => Ok(()),
            _ => Err(err),
        };
    }

    let metadata = stdfs::symlink_metadata(temp.path())?;

    if !expected(&metadata) {
        // The entry is now sitting at the temp path under our control. Put it
        // back where it came from; only if that fails does the drop guard get
        // to delete it, which is the same choice the previous code made.
        if let Some(error) = stdfs::rename(temp.path(), path).err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path is not a {what} and could not restore original entry: {error}"),
            ));
        }

        // The entry is back where it belongs, so the temp name is free again.
        let _ = temp.keep();
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a {what}"),
        ));
    }

    // The caller asked for the entry to be gone, and a failure to remove it is
    // the caller's business, so this stays an explicit fallible step rather
    // than being left to the guard, whose own removal ignores errors.
    stdfs::remove_file(temp.path())?;
    Ok(())
}

/// A temporary file that deletes itself when it goes out of scope.
///
/// The atomic-write path creates a temp file and then runs several safety
/// checks before renaming it over the real destination. Every one of those
/// checks can bail out, and each bail-out has to delete the temp file first.
/// Spelled out by hand that was four removals in one function and five in the
/// other — one of them written longhand instead of calling the local helper —
/// so adding an early return later could silently leave a
/// `.<prefix>.<random>.tmp` file in the user's config or state directory.
///
/// With a guard the deletion happens on every exit from the scope, including a
/// panic, and the one path that must *not* delete (the successful rename, which
/// has already moved the file away) opts out by calling [`TempFile::keep`].
struct TempFile {
    path: PathBuf,
    /// `false` once the file is no longer ours to remove.
    armed: bool,
}

impl TempFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Stop guarding the file and hand its path back.
    ///
    /// Called immediately after the rename that turns the temp file into the
    /// real destination file: at that point nothing exists at the temp path any
    /// more, and blindly removing that name could delete a file some other
    /// process has since created there.
    fn keep(mut self) -> PathBuf {
        self.armed = false;
        std::mem::take(&mut self.path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = stdfs::remove_file(&self.path);
        }
    }
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

/// The effective user id of this process.
///
/// Two things depend on this being the *real* answer and not a guess:
/// `is_private_dir_metadata` decides a directory belongs to us by comparing its
/// owner against this value, and `ipc::socket_path` builds the default socket
/// path `/tmp/obsctl-{uid}/obsctl.sock` from it. An earlier version inferred the
/// id by stat-ing `/proc/self` and, when that was unavailable, the *current
/// working directory* — so a user whose shell sat in a world-owned directory on
/// a system without procfs was reported as uid 0 (root). Both the ownership
/// check and the socket path were then computed from the same wrong number, so
/// they agreed with each other and the guard passed. `geteuid()` is the kernel
/// answering about this process; it cannot fail and has no fallback path.
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` reads a field of the calling process. It takes no
        // arguments, touches no memory we own, and is documented as always
        // succeeding, so there is no error case and nothing to invalidate.
        unsafe { libc::geteuid() }
    }

    #[cfg(not(unix))]
    {
        0
    }
}

/// Create a fresh, private temporary file in `dir`.
///
/// Returns the open handle together with a [`TempFile`] guard: the file is
/// removed as soon as the guard is dropped, so a caller only has to remember
/// the *one* case where it should survive.
fn create_secure_temp_file_in(
    dir: &Path,
    name_prefix: &str,
    mode: u32,
) -> io::Result<(File, TempFile)> {
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
                let guard = TempFile::new(tmp_path);

                #[cfg(unix)]
                return Ok((file, guard));

                #[cfg(not(unix))]
                {
                    // On platforms where `OpenOptions::mode` is unavailable the
                    // permissions are applied after the fact; failing that, the
                    // guard removes the file as the scope ends.
                    secure_permissions(guard.path(), mode)?;
                    return Ok((file, guard));
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
    let parent = path.parent().ok_or_else(no_parent)?;

    ensure_private_parent(path)?;

    // `tmp_guard` removes the temporary file on every path out of this function
    // except the successful rename below, which disarms it explicitly.
    let (mut tmp, tmp_guard) = create_secure_temp_file_in(parent, tmp_prefix, mode)?;

    if let Err(err) = write(&mut tmp) {
        drop(tmp);
        return Err(err);
    }
    tmp.flush()?;
    tmp.sync_all()?;
    drop(tmp);

    // Re-check destination safety immediately before the rename.
    ensure_atomic_target_safe(path, parent)?;

    match stdfs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Err(not_regular_file()),
        Ok(_) if !allow_overwrite => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    stdfs::rename(tmp_guard.path(), path)?;
    // The temporary file is now the destination file; stop guarding the name it
    // used to occupy.
    let _ = tmp_guard.keep();
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

        assert_eq!(count_temp_files(dir.path(), "obsctl-test"), 0);
    }

    #[cfg(unix)]
    #[test]
    fn create_secure_temp_file_in_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let (tmp, guard) = create_secure_temp_file_in(dir.path(), "obsctl-perm", 0o600).unwrap();
        let mode = tmp.metadata().unwrap().permissions().mode();
        drop(tmp);
        assert_eq!(mode & 0o777, 0o600);
        let path = guard.path().to_path_buf();
        assert!(path.exists());
        drop(guard);
        assert!(!path.exists(), "dropping the guard must remove the file");
    }

    #[cfg(unix)]
    #[test]
    fn kept_temp_file_survives_the_guard() {
        let dir = tempdir().unwrap();
        let (tmp, guard) = create_secure_temp_file_in(dir.path(), "obsctl-keep", 0o600).unwrap();
        drop(tmp);

        let path = guard.keep();
        assert!(path.exists(), "keep() must disarm the removal");
        let _ = std::fs::remove_file(&path);
    }

    /// Count leftover `.<prefix>.<random>.tmp` files in `dir`.
    ///
    /// Several tests assert that a failed atomic write leaves nothing behind.
    /// Each used to spell out the same read_dir/starts_with/ends_with loop.
    fn count_temp_files(dir: &Path, prefix: &str) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .filter(|entry| {
                let name = entry.as_ref().unwrap().file_name();
                let name = name.to_string_lossy();
                name.starts_with(&format!(".{prefix}.")) && name.ends_with(".tmp")
            })
            .count()
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
        assert_eq!(count_temp_files(dir.path(), "obsctl-unsafe-link"), 0);
    }

    #[test]
    fn write_atomic_with_temp_file_cleans_temp_on_write_failure() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.yml");
        let _ = write_atomic_with_temp_file(&path, "obsctl-temp-cleanup", 0o600, true, |_tmp| {
            Err(std::io::Error::other("boom"))
        })
        .err();

        assert_eq!(count_temp_files(dir.path(), "obsctl-temp-cleanup"), 0);
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

        assert_eq!(count_temp_files(dir.path(), "obsctl-remove-guard"), 0);
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
