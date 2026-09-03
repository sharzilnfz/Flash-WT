use std::path::PathBuf;
use std::time::Duration;

use clap::{ArgGroup, Parser, Subcommand};

fn parse_age_value(text: &str) -> Result<Duration, String> {
    crate::gc::parse_age(text)
        .ok_or_else(|| format!("invalid duration {text:?} (try 0s, 90s, 10m, 1h, 7d)"))
}

#[derive(Parser)]
#[command(
    name = "flashwt",
    version,
    about = "Instant git worktrees with heavy directories already hydrated"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: FlashwtCommand,
}

#[derive(Subcommand)]
pub enum FlashwtCommand {
    Hydrate {
        path: PathBuf,

        #[arg(long)]
        source: Option<PathBuf>,

        #[arg(long)]
        manifest: Option<PathBuf>,
    },

    Init {
        #[arg(long)]
        dir: Option<PathBuf>,

        #[arg(long, short)]
        force: bool,
    },

    Clean {
        name: Option<String>,

        #[arg(long)]
        dir: Option<PathBuf>,

        #[arg(long)]
        all: bool,

        #[arg(long, short)]
        force: bool,

        #[arg(long, value_parser = parse_age_value)]
        age: Option<Duration>,
    },

    #[command(
        alias = "new",
        long_about = "Create a worktree for NAME (used as the git branch \
name) and hydrate the heavy directories listed in the .flashwtinclude manifest.

Hydrated files are private, fully writable copy-on-write clones of store
blobs (fclonefileat on macOS); they share the store's physical blocks until
first write. Filesystems that refuse clones fall back to plain byte copies.

FLASHWT_HARDLINK=1 opts into EXPERIMENTAL hardlinked materialization for maximum
space sharing: linked files share the store's inode, which must be made
read-only, so tools that rewrite hydrated files in place fail loudly with
permission errors. FLASHWT_NO_HARDLINK forces plain byte copies instead. For all
flashwt flags: set to 0 to disable, to anything else (including 1) to enable.

Blobs are hash-verified once and then trusted while their size and mtime
stay unchanged (a verified-blob ledger beside the store tracks this);
FLASHWT_VERIFY=1 forces a full re-hash of every blob on every run for paranoid
verification.

GC bookkeeping: each successful create publishes one store-local mirror
(<store>/worktrees/) naming the blobs it hydrates from. FLASHWT_TIMING=1
prints per-stage timings (`flashwt-stage ingest=...` and friends) to stderr.

Whole-directory snapshots are automatically enabled by default on macOS APFS.
FLASHWT_SNAPSHOTS=0 opts out and forces per-file hydration. FLASHWT_VERIFY=1 bypasses
snapshot hits entirely and rebuilds from freshly hashed blobs."
    )]
    Create {
        name: String,

        #[arg(long)]
        base: Option<String>,

        #[arg(long)]
        manifest: Option<PathBuf>,

        #[arg(long)]
        dir: Option<PathBuf>,
    },

    Remove {
        name: String,

        #[arg(long)]
        dir: Option<PathBuf>,
    },

    Sweep {
        #[arg(long, value_parser = parse_age_value)]
        age: Option<Duration>,

        #[arg(long)]
        dry_run: bool,
    },

    Scrub {
        #[arg(long)]
        dry_run: bool,
    },

    Store {
        #[command(subcommand)]
        action: StoreAction,
    },

    Doctor,

    #[command(alias = "isolate")]
    Scratch {
        name: Option<String>,

        #[arg(long)]
        manifest: Option<PathBuf>,

        #[arg(long)]
        dir: Option<PathBuf>,

        #[arg(long)]
        run: Option<String>,

        #[arg(long, value_parser = parse_age_value)]
        ttl: Option<Duration>,
    },

    #[command(name = "list", alias = "ls")]
    List,

    #[command(alias = "test-drive")]
    Demo,

    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    Lease {
        #[command(subcommand)]
        action: Option<LeaseAction>,
    },
}

impl FlashwtCommand {
    pub fn name(&self) -> &'static str {
        match self {
            FlashwtCommand::Hydrate { .. } => "hydrate",
            FlashwtCommand::Init { .. } => "init",
            FlashwtCommand::Clean { .. } => "clean",
            FlashwtCommand::Create { .. } => "create",
            FlashwtCommand::Remove { .. } => "remove",
            FlashwtCommand::Sweep { .. } => "sweep",
            FlashwtCommand::Scrub { .. } => "scrub",
            FlashwtCommand::Store { .. } => "store",
            FlashwtCommand::Doctor => "doctor",
            FlashwtCommand::Scratch { .. } => "scratch",
            FlashwtCommand::List => "list",
            FlashwtCommand::Demo => "demo",
            FlashwtCommand::Completions { .. } => "completions",
            FlashwtCommand::Lease { .. } => "lease",
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum LeaseAction {
    #[command(alias = "list", alias = "ls")]
    Show {
        id: Option<String>,

        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum StoreAction {
    #[command(name = "du", alias = "disk-usage")]
    Du,

    #[command(group = ArgGroup::new("migrate-mode")
        .required(true)
        .args(&["activate_mark_sweep", "drop_legacy_refs"]))]
    Migrate {
        #[arg(long)]
        activate_mark_sweep: bool,

        #[arg(long)]
        drop_legacy_refs: bool,
    },
}
