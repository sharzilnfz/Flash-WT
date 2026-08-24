//! Command dispatch (arch-hardening ticket 03): one thin handler per
//! subcommand, wired here.

pub mod create;
pub mod migrate;
pub mod remove;
pub mod sweep;

use crate::cli::{StoreAction, WtCommand};
use crate::error::Result;

pub fn run(command: WtCommand) -> Result<()> {
    match command {
        WtCommand::Create {
            name,
            manifest,
            dir,
        } => create::run(&name, manifest.as_deref(), dir.as_deref()),
        WtCommand::Remove { name, dir } => remove::run(&name, dir.as_deref()),
        WtCommand::Sweep { age } => sweep::run(age),
        WtCommand::Store { action } => match action {
            StoreAction::Migrate {
                activate_mark_sweep,
                drop_legacy_refs,
            } => migrate::run(activate_mark_sweep, drop_legacy_refs),
        },
    }
}
