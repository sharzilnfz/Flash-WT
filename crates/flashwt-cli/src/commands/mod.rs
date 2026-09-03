pub mod clean;
pub mod completions;
pub mod create;
pub mod demo;
pub mod doctor;
pub mod hydrate;
pub mod init;
pub mod lease;
pub mod list;
pub mod scratch;
pub mod scrub;
pub mod sweep;

use crate::cli::{FlashwtCommand, StoreAction};
use crate::config::RunConfig;
use crate::envelope::Envelope;
use crate::error::{Error, Result};
use crate::gc;

fn emit_json<T: serde::Serialize>(
    command_name: &str,
    data: T,
    diags: Vec<crate::envelope::Diagnostic>,
) -> Result<()> {
    let env = Envelope::ok(command_name, data, diags);
    println!(
        "{}",
        serde_json::to_string(&env).map_err(|e| Error::Store(e.to_string()))?
    );
    Ok(())
}

pub fn run(command: FlashwtCommand, cfg: &RunConfig) -> Result<Option<i32>> {
    let command_name = command.name();
    match command {
        FlashwtCommand::List => {
            let (data, diags) = list::run(cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Doctor => {
            let (data, diags) = doctor::run(cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Hydrate {
            path,
            source,
            manifest,
        } => {
            let (data, diags) = hydrate::run(&path, source.as_deref(), manifest.as_deref(), cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Init { dir, force } => {
            let (data, diags) = init::run(dir.as_deref(), force, cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Create {
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
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Clean {
            name,
            dir,
            all,
            force,
            age,
        } => {
            let (data, diags) = clean::run(name.as_deref(), dir.as_deref(), all, force, age, cfg)?;
            let has_errors = diags.iter().any(|d| d.level.as_deref() == Some("error"));
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            if has_errors { Ok(Some(1)) } else { Ok(None) }
        }
        FlashwtCommand::Remove { name, dir } => {
            let (data, diags) = gc::remove(&name, dir.as_deref(), cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Sweep { age, dry_run } => {
            let (data, diags) = sweep::run(age, dry_run, cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Scrub { dry_run } => {
            let (data, diags) = scrub::run(dry_run, cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Store { action } => match action {
            StoreAction::Du => {
                let (data, diags) = doctor::store_du(cfg)?;
                if cfg.json {
                    emit_json("store du", data, diags)?;
                }
                Ok(None)
            }
            StoreAction::Migrate {
                activate_mark_sweep,
                drop_legacy_refs,
            } => {
                let (data, diags) = gc::migrate(activate_mark_sweep, drop_legacy_refs, cfg)?;
                if cfg.json {
                    emit_json(command_name, data, diags)?;
                }
                Ok(None)
            }
        },
        FlashwtCommand::Scratch {
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
                emit_json(command_name, data, diags)?;
            }
            Ok(exit_code)
        }
        FlashwtCommand::Demo => {
            let (data, diags) = demo::run(cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
        FlashwtCommand::Completions { shell } => {
            completions::run(shell)?;
            Ok(None)
        }
        FlashwtCommand::Lease { action } => {
            let (data, diags) = lease::run(action, cfg)?;
            if cfg.json {
                emit_json(command_name, data, diags)?;
            }
            Ok(None)
        }
    }
}
