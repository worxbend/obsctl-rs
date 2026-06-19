pub mod cli;
pub mod config;
pub mod domain;
pub mod ipc;
pub mod obs;
pub mod runtime;
pub mod server;
pub mod service;
pub mod support;
pub mod tui;

pub fn run() -> i32 {
    use clap::Parser as _;
    let cli = cli::args::Cli::parse();
    cli::router::run(cli)
}
