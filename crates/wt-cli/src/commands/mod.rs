//! Command dispatch (arch-hardening ticket 03): one thin handler per
//! subcommand, wired here.

pub mod clean;
pub mod completions;
pub mod create;
pub mod demo;
pub mod list;
pub mod migrate;
pub mod remove;
pub mod scratch;
pub mod scrub;
pub mod sweep;

use crate::cli::{StoreAction, WtCommand};
use crate::config::RunConfig;
use crate::envelope::Envelope;
use crate::error::{Error, Result};

pub fn run(command: WtCommand, cfg: &RunConfig) -> Result<Option<i32>> {
    let command_name = command.name();
    match command {
        WtCommand::List => {
            let (data, diags) = list::run(cfg)?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::New {
            name,
            base,
            manifest,
            dir,
        }
        | WtCommand::Create {
            name,
            base,
            manifest,
            dir,
        } => {
            let (data, diags) = create::run(
                &name,
                base.as_deref(),
                manifest.as_deref(),
                dir.as_deref(),
                cfg,
            )?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::Clean {
            name,
            dir,
            all,
            force,
            age,
        } => {
            let (data, diags) = clean::run(
                name.as_deref(),
                dir.as_deref(),
                all,
                force,
                age,
                cfg,
            )?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::Remove { name, dir } => {
            let (data, diags) = remove::run(&name, dir.as_deref(), cfg)?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::Sweep { age } => {
            let (data, diags) = sweep::run(age, cfg)?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::Scrub { dry_run } => {
            let (data, diags) = scrub::run(dry_run, cfg)?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::Store { action } => match action {
            StoreAction::Migrate {
                activate_mark_sweep,
                drop_legacy_refs,
            } => {
                let (data, diags) = migrate::run(activate_mark_sweep, drop_legacy_refs, cfg)?;
                if cfg.json {
                    let env = Envelope::ok(command_name, data, diags);
                    println!(
                        "{}",
                        serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                    );
                }
                Ok(None)
            }
        },
        WtCommand::Scratch {
            name,
            manifest,
            dir,
            run: run_cmd,
            ttl,
        }
        | WtCommand::Isolate {
            name,
            manifest,
            dir,
            run: run_cmd,
            ttl,
        } => {
            let (data, diags, exit_code) = scratch::run(
                name.as_deref(),
                manifest.as_deref(),
                dir.as_deref(),
                run_cmd.as_deref(),
                ttl,
                cfg,
            )?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(exit_code)
        }
        WtCommand::Demo | WtCommand::TestDrive => {
            let (data, diags) = demo::run(cfg)?;
            if cfg.json {
                let env = Envelope::ok(command_name, data, diags);
                println!(
                    "{}",
                    serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
                );
            }
            Ok(None)
        }
        WtCommand::Completions { shell } => {
            completions::run(shell)?;
            Ok(None)
        }
    }
}
