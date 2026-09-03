use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;
use crate::error::Result;

pub fn run(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    clap_complete::generate(shell, &mut command, "flashwt", &mut std::io::stdout());
    Ok(())
}
