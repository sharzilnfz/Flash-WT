//! Shell completion generation (market-launch ticket 01): renders the
//! `wt` CLI definition into a completion script for the requested
//! shell, straight to stdout.

use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;
use crate::error::Result;

pub fn run(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "wt", &mut std::io::stdout());
    Ok(())
}
