pub mod cli;
pub mod config;
pub mod domain;
pub mod ipc;
pub mod localization;
pub mod obs;
pub mod runtime;
pub mod server;
pub mod service;
pub mod support;
pub mod tui;

// Embeds `locales/en.yml` into the binary at compile time as the ultimate
// fallback, and layers `localization::FileBackend` on top so runtime locale
// override files (see `localization` module docs) take priority when present.
rust_i18n::i18n!(
    "locales",
    fallback = "en",
    backend = localization::FileBackend::new()
);

pub fn run() -> i32 {
    use clap::Parser as _;
    let cli = cli::args::Cli::parse();
    cli::router::run(cli)
}
