use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    pub headless: bool,
    pub config_path: Option<PathBuf>,
}
