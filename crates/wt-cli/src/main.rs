//! `wt` — instant git worktrees with heavy directories already
//! hydrated. This file is only the entry point: command definitions
//! live in `cli.rs`, dispatch in `commands/`, and the machinery in
//! `hydrate`, `manifest`, `snapshots`, `gc`, `gitops`, and `timing`.

mod cli;
mod commands;
mod config;
mod envelope;
mod error;
mod gc;
mod gitops;
mod hydrate;
mod manifest;
mod snapshots;
mod timing;
mod toolchain;

use clap::Parser;

use cli::Cli;
use config::RunConfig;

fn main() {
    let cli = Cli::parse();
    let command_name = cli.command.name();
    // Env policy is parsed exactly once, here, and threaded through
    // the machinery as data.
    let mut cfg = RunConfig::from_env();
    cfg.json = cli.json;
    if let Err(e) = commands::run(cli.command, &cfg) {
        if cfg.json {
            let env = envelope::Envelope::<()>::error(
                command_name,
                vec![envelope::Diagnostic::error("ERROR", e.to_string())],
            );
            if let Ok(json) = serde_json::to_string(&env) {
                println!("{json}");
            }
        } else {
            eprintln!("wt: {e}");
        }
        // Usage mistakes exit 2 like clap's own parse errors; every
        // other failure exits 1.
        std::process::exit(match e {
            error::Error::Usage(_) => 2,
            _ => 1,
        });
    }
}
