//! `wt` — instant git worktrees with heavy directories already
//! hydrated. This file is only the entry point: command definitions
//! live in `cli.rs`, dispatch in `commands/`, and the machinery in
//! `hydrate`, `manifest`, `snapshots`, `gc`, `gitops`, and `timing`.

mod cli;
mod commands;
mod config;
mod error;
mod gc;
mod gitops;
mod hydrate;
mod manifest;
mod snapshots;
mod timing;

use clap::Parser;

use cli::Cli;
use config::RunConfig;

fn main() {
    let command = Cli::parse().command;
    // Env policy is parsed exactly once, here, and threaded through
    // the machinery as data.
    let cfg = RunConfig::from_env();
    if let Err(e) = commands::run(command, &cfg) {
        eprintln!("wt: {e}");
        // Usage mistakes exit 2 like clap's own parse errors; every
        // other failure exits 1.
        std::process::exit(match e {
            error::Error::Usage(_) => 2,
            _ => 1,
        });
    }
}
