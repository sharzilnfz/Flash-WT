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
pub mod output;
pub mod receipt;
pub mod signal;
pub mod timing;
pub mod toolchain;
pub mod workspace;

use clap::Parser;
use cli::Cli;
use config::RunConfig;

pub fn run_cli() {
    signal::init_signal_handlers();
    let cli = Cli::parse();
    let command_name = cli.command.name();

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
                eprintln!("flashwt: {e}");
            }

            std::process::exit(match e {
                error::Error::Usage(_) => 2,
                _ => 1,
            });
        }
    }
}
