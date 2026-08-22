use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "wt",
    version,
    about = "Instant git worktrees with heavy directories already hydrated"
)]
struct Cli {
    #[command(subcommand)]
    command: WtCommand,
}

#[derive(Subcommand)]
enum WtCommand {
    /// Create a worktree for NAME (used as the git branch name) and
    /// hydrate the heavy directories listed in the .wtinclude manifest.
    Create {
        /// Branch name; also names the new worktree directory.
        name: String,
        /// Manifest listing heavy directories (gitignore syntax).
        /// Defaults to `.wtinclude` in the repository root.
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Destination for the new worktree. Defaults to a sibling of
        /// the current repository named `<repo>-<name>`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("not inside a git repository".into());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn create(name: &str, manifest: Option<&Path>, dir: Option<&Path>) -> Result<(), String> {
    let _ = manifest;
    let root = repo_root()?;
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => root
            .parent()
            .ok_or("repository root has no parent")?
            .join(format!(
                "{}-{name}",
                root.file_name()
                    .ok_or("cannot name repository directory")?
                    .to_string_lossy()
            )),
    };
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }

    let added = {
        let mut cmd = Command::new("git");
        cmd.current_dir(&root)
            .args(["worktree", "add", "-b", name])
            .arg(&dest)
            .arg("HEAD");
        run(&mut cmd)
    }
    .or_else(|_| {
        let mut cmd = Command::new("git");
        cmd.current_dir(&root)
            .args(["worktree", "add"])
            .arg(&dest)
            .arg(name);
        run(&mut cmd)
    });
    added?;

    println!(
        "created worktree {} from {}",
        dest.display(),
        root.display()
    );
    println!("hydration: no backends wired yet (ticket 02)");
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        WtCommand::Create {
            name,
            manifest,
            dir,
        } => create(&name, manifest.as_deref(), dir.as_deref()),
    };
    if let Err(msg) = result {
        eprintln!("wt: {msg}");
        std::process::exit(1);
    }
}
