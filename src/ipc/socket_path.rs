use std::path::PathBuf;

pub fn default_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir)
            .join("obsctl")
            .join("obsctl.sock");
    }
    let uid = current_uid();
    PathBuf::from(format!("/tmp/obsctl-{uid}/obsctl.sock"))
}

fn current_uid() -> u32 {
    std::fs::metadata("/proc/self")
        .or_else(|_| std::fs::metadata("."))
        .map(|m| {
            use std::os::unix::fs::MetadataExt as _;
            m.uid()
        })
        .unwrap_or(0)
}
