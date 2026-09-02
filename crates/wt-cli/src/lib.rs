//! `wt-cli` library internals.

#![allow(missing_docs)]

pub mod base;
pub mod cli;
pub mod commands;
pub mod config;
pub mod envelope;
pub mod error;
pub mod gc;
pub mod hydrate;
pub mod hydration_filter;
pub mod manifest;
pub mod output;
pub mod signal;
pub mod timing;
pub mod toolchain;
pub mod workspace;

use clap::Parser;
use cli::Cli;
use config::RunConfig;

/// CLI entrypoint logic shared across `wt-hydrate` and `wt` binaries.
pub fn run_cli() {
    signal::init_signal_handlers();
    let cli = Cli::parse();
    let command_name = cli.command.name();
    // Env policy is parsed exactly once, here, and threaded through
    // the machinery as data.
    let mut cfg = RunConfig::from_env();
    cfg.json = cli.json;
    match commands::run(cli.command, &cfg) {
        Ok(Some(code)) if code != 0 => {
            std::process::exit(code);
        }
        Ok(_) => {}
        Err(e) => {
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
}
