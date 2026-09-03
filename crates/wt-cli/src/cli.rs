//! Command-line surface of the `wt` binary (decomposed from main.rs
//! by arch-hardening ticket 03). Definitions only — behavior lives in
//! the command handlers under `commands/`.

use std::path::PathBuf;
use std::time::Duration;

use clap::{ArgGroup, Parser, Subcommand};

/// Custom `--age` value parser: bad durations die at parse time,
/// before any side effect.
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
    /// Emit machine-readable JSON output on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: WtCommand,
}

#[derive(Subcommand)]
pub enum WtCommand {
    /// Hydrate heavy directories into an existing directory or worktree.
    Hydrate {
        /// Destination directory or worktree to hydrate.
        path: PathBuf,
        /// Source repository root containing the heavy directories to ingest.
        /// Defaults to discovering the enclosing repository.
        #[arg(long)]
        source: Option<PathBuf>,
        /// Manifest listing heavy directories (gitignore syntax).
        /// Defaults to `.wtinclude` in the source repository root.
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    /// Initialize a starter `.wtinclude` manifest in the repository root or target directory.
    Init {
        /// Target directory to write `.wtinclude` into.
        /// Defaults to the repository root.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Overwrite existing `.wtinclude` manifest if it exists.
        #[arg(long, short)]
        force: bool,
    },
    /// Clean and remove worktrees, reclaiming unreferenced store storage (modern primary verb).
    Clean {
        /// Branch name to remove. If omitted, interactively prompts or batches cleanup.
        name: Option<String>,
        /// Path of the worktree to remove. Defaults to the sibling `<repo>-<name>`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Remove all stale/merged worktrees non-interactively.
        #[arg(long)]
        all: bool,
        /// Force removal of unmerged worktrees.
        #[arg(long, short)]
        force: bool,
        /// Minimum age threshold for GC sweep.
        #[arg(long, value_parser = parse_age_value)]
        age: Option<Duration>,
    },
    /// Create a worktree for NAME (used as the git branch name) and
    /// hydrate the heavy directories listed in the .wtinclude manifest.
    #[command(
        alias = "new",
        long_about = "Create a worktree for NAME (used as the git branch \
name) and hydrate the heavy directories listed in the .wtinclude manifest.

Hydrated files are private, fully writable copy-on-write clones of store
blobs (fclonefileat on macOS); they share the store's physical blocks until
first write. Filesystems that refuse clones fall back to plain byte copies.

WT_HARDLINK=1 opts into EXPERIMENTAL hardlinked materialization for maximum
space sharing: linked files share the store's inode, which must be made
read-only, so tools that rewrite hydrated files in place fail loudly with
permission errors. WT_NO_HARDLINK forces plain byte copies instead. For all
wt flags: set to 0 to disable, to anything else (including 1) to enable.

Blobs are hash-verified once and then trusted while their size and mtime
stay unchanged (a verified-blob ledger beside the store tracks this);
WT_VERIFY=1 forces a full re-hash of every blob on every run for paranoid
verification.

GC bookkeeping: each successful create publishes one store-local mirror
(<store>/worktrees/) naming the blobs it hydrates from. WT_TIMING=1
prints per-stage timings (`wt-stage ingest=...` and friends) to stderr.

Whole-directory snapshots are automatically enabled by default on macOS APFS.
WT_SNAPSHOTS=0 opts out and forces per-file hydration. WT_VERIFY=1 bypasses
snapshot hits entirely and rebuilds from freshly hashed blobs."
    )]
    Create {
        /// Branch name; also names the new worktree directory.
        name: String,
        /// Base branch or ref to create the worktree from (records symbolic tracking).
        #[arg(long)]
        base: Option<String>,
        /// Manifest listing heavy directories (gitignore syntax).
        /// Defaults to `.wtinclude` in the repository root.
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Destination for the new worktree. Defaults to a sibling of
        /// the current repository named `<repo>-<name>`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Remove a worktree and release the store references its
    /// hydration claimed (recorded in the wt-hydrated.tsv ledger).
    Remove {
        /// Branch name; also names the worktree directory, unless
        /// --dir says otherwise.
        name: String,
        /// Path of the worktree to remove. Defaults to the sibling
        /// `<repo>-<name>` that `wt create` produces.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Delete store entries no live worktree references and older
    /// than --age. Entries a live worktree references are never
    /// touched. In mark-sweep mode (see `wt store migrate`) liveness
    /// comes from store mirrors plus the grace period instead of
    /// refcounts.
    Sweep {
        /// Minimum age of an unreferenced entry before it may be
        /// deleted (e.g. 0s, 90s, 10m, 24h, 7d). The floor protects
        /// content that is mid-ingestion or awaiting its first
        /// reference. Defaults to 7d in legacy mode, and to
        /// WT_GC_GRACE (default 15m) in mark-sweep mode.
        #[arg(long, value_parser = parse_age_value)]
        age: Option<Duration>,
        /// Perform mark-and-sweep analysis without deleting files.
        #[arg(long)]
        dry_run: bool,
    },
    /// Re-hash every blob in the store against its content address
    /// and repair corruption. Closes the documented trust-model gap:
    /// a bit flip that preserves both size and mtime slips past the
    /// verified-blob ledger between checks, and only a full scrub can
    /// see it.
    Scrub {
        /// Report corrupt blobs without deleting anything (and
        /// without touching the verified-blob ledger).
        #[arg(long)]
        dry_run: bool,
    },
    /// Store-level inspection and one-way migrations.
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
    /// Inspect environment variables, store configuration, filesystem capabilities, and disk usage.
    Doctor,
    /// Create an ephemeral scratch worktree with lease persistence.
    /// Optionally execute a command inside the sandbox and clean up on exit.
    #[command(alias = "isolate")]
    Scratch {
        /// Optional branch/worktree name (defaults to auto-generated scratch-<id>).
        name: Option<String>,
        /// Manifest listing heavy directories (.wtinclude).
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Destination for the worktree.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Command to execute inside the sandbox.
        #[arg(long)]
        run: Option<String>,
        /// Lease time-to-live duration (e.g. 15m, 1h, 24h).
        #[arg(long, value_parser = parse_age_value)]
        ttl: Option<Duration>,
    },
    /// List active git worktrees with disk usage and shared savings.
    #[command(name = "list", alias = "ls")]
    List,
    /// Run an end-to-end zero-setup demonstration and benchmark:
    /// creates a synthetic 10,000-file project fixture, measures baseline copy
    /// vs. wt CoW hydration, validates mutation isolation, and cleans up.
    #[command(alias = "test-drive")]
    Demo,
    /// Generate a shell completion script for the given shell.
    /// Source the output (or drop it in a completion directory) to
    /// enable tab-completion of subcommands and flags.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Inspect active ephemeral scratch leases.
    Lease {
        #[command(subcommand)]
        action: Option<LeaseAction>,
    },
}

impl WtCommand {
    /// Return the canonical command name for JSON envelopes and logs.
    pub fn name(&self) -> &'static str {
        match self {
            WtCommand::Hydrate { .. } => "hydrate",
            WtCommand::Init { .. } => "init",
            WtCommand::Clean { .. } => "clean",
            WtCommand::Create { .. } => "create",
            WtCommand::Remove { .. } => "remove",
            WtCommand::Sweep { .. } => "sweep",
            WtCommand::Scrub { .. } => "scrub",
            WtCommand::Store { .. } => "store",
            WtCommand::Doctor => "doctor",
            WtCommand::Scratch { .. } => "scratch",
            WtCommand::List => "list",
            WtCommand::Demo => "demo",
            WtCommand::Completions { .. } => "completions",
            WtCommand::Lease { .. } => "lease",
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum LeaseAction {
    /// Show active scratch worktree leases in machine-readable JSON format.
    #[command(alias = "list", alias = "ls")]
    Show {
        /// Optional lease ID or scratch branch name to inspect.
        id: Option<String>,
        /// Include expired or dead leases.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum StoreAction {
    /// Print breakdown of disk usage in the store.
    #[command(name = "du", alias = "disk-usage")]
    Du,
    /// Migrate the store's garbage-collection scheme (one-way; see
    /// ADR-0004). Until activated, sweep stays refcount-driven and
    /// every sweep audits mirrors against refs for parity.
    #[command(group = ArgGroup::new("migrate-mode")
        .required(true)
        .args(&["activate_mark_sweep", "drop_legacy_refs"]))]
    Migrate {
        /// Sweep collects from live-mirror marks plus the grace
        /// period (WT_GC_GRACE, default 15m) from now on. Legacy
        /// refs/ files stay maintained by create/remove so pre-change
        /// binaries remain safe, but are ignored for liveness.
        #[arg(long)]
        activate_mark_sweep: bool,
        /// Drop ALL legacy refcount files and stop writing new ones.
        /// Pre-cutover binaries must not use this store afterwards;
        /// this is loud, explicit, and irreversible.
        #[arg(long)]
        drop_legacy_refs: bool,
    },
}
