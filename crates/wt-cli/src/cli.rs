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
    name = "wt",
    version,
    about = "Instant git worktrees with heavy directories already hydrated"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: WtCommand,
}

#[derive(Subcommand)]
pub enum WtCommand {
    /// Create a worktree for NAME (used as the git branch name) and
    /// hydrate the heavy directories listed in the .wtinclude manifest.
    #[command(long_about = "Create a worktree for NAME (used as the git branch \
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

WT_SNAPSHOTS=1 (macOS/APFS, opt-in) hydrates each heavy directory by
one recursive clonefile(2) from a whole-directory snapshot in the store
when one matches: hits cost no per-file work. Misses build and publish
a snapshot first. WT_VERIFY=1 bypasses snapshot hits entirely and
rebuilds from freshly hashed blobs. Filesystems without clone support,
and clone refusals like cross-device destinations, fall back to the
per-file ladder above.")]
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
}

#[derive(Subcommand)]
pub enum StoreAction {
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
