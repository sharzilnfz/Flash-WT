//! Whole-directory snapshots (fast-hydration ticket 08, Phase 2 of
//! AGENT_HANDOFF_PLAN_REVISED.md).
//!
//! One snapshot per heavy directory lives under
//! `<root>/snapshots/<64-hex-manifest-hash>/`:
//!
//! ```text
//! manifest.tsv        canonical manifest (see [`manifest`])
//! .complete           schema version + manifest hash
//! tree/               the hydrated tree: regular files hardlinked
//!                     to object blobs
//! ```
//!
//! The tree sits under `tree/` rather than beside the metadata so a
//! single recursive `clonefile(2)` of `tree/` produces EXACTLY the
//! heavy directory — no metadata files leaking into the worktree
//! (layout deviation from the handoff sketch, recorded in ADR-0005).
//!
//! Snapshots are rebuildable caches, not GC roots: a snapshot survives
//! only while some live store mirror names it (ADR-0004). The tree's
//! files share inodes with object blobs until the whole tree is cloned
//! out with one APFS `clonefile(2)`, which hands the worktree fresh
//! private writable inodes.
//!
//! Integrity model: verification happens once, at publish. Every file
//! blob is proven (verified-ledger trust, or full read-and-hash under
//! `WT_VERIFY=1`) before it is linked into the new snapshot; after the
//! atomic rename the snapshot is trusted like any published blob. A
//! hit performs zero blob reads by design; callers wanting continuous
//! corruption detection set `paranoid`, which bypasses hits entirely
//! and rebuilds from freshly hashed blobs.
//!
//! Module layout:
//!
//! - [`manifest`]: the TSV codec — Manifest, SnapshotEntry,
//!   EntryKind, relpath validation.
//! - [`publish`]: the distributed publish protocol — staging dirs,
//!   atomic rename, winner-collision handling, phase timings.
//! - [`tree`]: staging-tree construction and the paranoid proof pass.

mod manifest;
mod publish;
mod tree;

#[cfg(test)]
mod tests;

pub use manifest::{EntryKind, Manifest, SnapshotEntry};
pub use publish::{BuildError, PublishOutcome, PublishReceipt, SnapshotBuildTiming};

use std::fs;
use std::path::{Path, PathBuf};

use crate::ContentId;

/// Final directory of one published snapshot:
/// `<root>/snapshots/<hash>/`.
pub fn snapshot_path(root: &Path, hash: &ContentId) -> PathBuf {
    root.join("snapshots").join(hash.to_string())
}

/// The clonable hydrated tree inside a published snapshot:
/// `<root>/snapshots/<hash>/tree/`. This — not the snapshot root,
/// which also carries `manifest.tsv` and `.complete` — is what gets
/// cloned into a worktree.
pub fn snapshot_tree_path(root: &Path, hash: &ContentId) -> PathBuf {
    snapshot_path(root, hash).join("tree")
}

/// Load and fully validate the published snapshot for `hash`, if one
/// exists. Valid means: parseable manifest whose header hash matches
/// the directory name, plus a `.complete` marker carrying the same
/// schema version and hash. Anything else is debris (GC collects it
/// after the grace period); callers treat `None` as a miss.
///
/// This is THE shared validity check — snapshot lookup, the
/// concurrent-publish loser, and mark-and-sweep all go through it.
pub fn read_published(root: &Path, hash: &ContentId) -> Option<Manifest> {
    let dir = snapshot_path(root, hash);
    let text = fs::read_to_string(dir.join("manifest.tsv")).ok()?;
    let manifest = Manifest::parse(&text).ok()?;
    if manifest.hash != *hash {
        return None;
    }
    let complete = fs::read_to_string(dir.join(".complete")).ok()?;
    let mut parts = complete.trim_end_matches('\n').split('\t');
    if parts.next()? != "v1" {
        return None;
    }
    if parts.next()? != hash.to_string() || parts.next().is_some() {
        return None;
    }
    Some(manifest)
}
