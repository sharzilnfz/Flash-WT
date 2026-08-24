//! `wt` — instant git worktrees with heavy directories already
//! hydrated. This file is only the entry point: command definitions
//! live in `cli.rs`, dispatch in `commands/`, and the machinery in
//! `hydrate`, `manifest`, `snapshots`, `gc`, `gitops`, and `timing`.

mod cli;
mod commands;
mod gc;
mod gitops;
mod hydrate;
mod manifest;
mod snapshots;
mod timing;

use clap::Parser;

use cli::Cli;

fn main() {
    let command = Cli::parse().command;
    if let Err(msg) = commands::run(command) {
        eprintln!("wt: {msg}");
        std::process::exit(1);
    }
}
