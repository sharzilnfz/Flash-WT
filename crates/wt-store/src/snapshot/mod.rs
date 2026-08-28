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
mod projection;
mod publish;
mod tree;

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;

pub use manifest::{EntryKind, Manifest, SnapshotEntry};
pub use projection::{SnapshotHydration, SnapshotOutcome, SnapshotProjectionEngine};
pub use publish::{BuildError, PublishOutcome, PublishReceipt, SnapshotBuildTiming};

use std::fs;
use std::io;
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
///
/// On IO errors this returns `None`, i.e. the snapshot is treated as
/// INVALID, which is the conservative direction: GC never marks
/// through an unreadable snapshot (its worktree holds private clones
/// and rebuilds on the next create), and a publisher never trusts one.
/// A permission-denied manifest is therefore indistinguishable from
/// debris to callers — accepted here because both readings lead to
/// "do not trust", never to data loss of blob truth. See
/// [`read_published_checked`] for the discriminating variant.
pub fn read_published(root: &Path, hash: &ContentId) -> Option<Manifest> {
    match read_published_checked(root, hash) {
        Verdict::Valid(manifest) => Some(manifest),
        Verdict::Miss | Verdict::Invalid(_) | Verdict::Io(_) => None,
    }
}

/// Why [`read_published`] said no. Private on purpose: every current
/// caller treats non-valid as "not usable", and keeping the enum in
/// this module means that policy lives in exactly one place. The
/// payload fields are carried for diagnostic completeness even though
/// no caller reads them yet.
#[allow(dead_code)]
enum Verdict {
    /// Nothing at the final name.
    Miss,
    /// Something is there but it fails validation; carries the reason.
    Invalid(String),
    /// The filesystem refused to say (permissions, EIO, ...).
    Io(io::Error),
    /// Fully valid; carries the parsed manifest.
    Valid(Manifest),
}

fn read_published_checked(root: &Path, hash: &ContentId) -> Verdict {
    let dir = snapshot_path(root, hash);
    let text = match fs::read_to_string(dir.join("manifest.tsv")) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Verdict::Miss,
        Err(e) => return Verdict::Io(e),
    };
    let manifest = match Manifest::parse(&text) {
        Ok(manifest) => manifest,
        Err(reason) => return Verdict::Invalid(format!("unparsable manifest: {reason}")),
    };
    if manifest.hash != *hash {
        return Verdict::Invalid("manifest hash does not match directory name".into());
    }
    let complete = match fs::read_to_string(dir.join(".complete")) {
        Ok(complete) => complete,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Verdict::Invalid("missing .complete marker".into());
        }
        Err(e) => return Verdict::Io(e),
    };
    let mut parts = complete.trim_end_matches('\n').split('\t');
    if parts.next() != Some("v1") {
        return Verdict::Invalid("wrong schema version in .complete".into());
    }
    let hash_text = hash.to_string();
    if parts.next() != Some(hash_text.as_str()) || parts.next().is_some() {
        return Verdict::Invalid(".complete does not match the manifest hash".into());
    }
    Verdict::Valid(manifest)
}
