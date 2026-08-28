//! Whole-directory snapshot hydration (fast-hydration ticket 08,
//! Phase 2 of AGENT_HANDOFF_PLAN_REVISED.md).
//!
//! Re-exports and delegates to [`wt_store::snapshot::SnapshotProjectionEngine`].

use std::path::Path;

use wt_store::{ContentId, DiskStore};
#[allow(unused_imports)]
pub use wt_store::{SnapshotHydration, SnapshotOutcome, SnapshotProjectionEngine};

use crate::config::RunConfig;
use crate::hydrate::Ingested;

/// Alias for backwards compatibility within CLI modules.
pub type Outcome = SnapshotOutcome;

/// Attempt an O(1) fast-path snapshot hydration without walking or ingesting
/// the heavy directory when a pinned lockfile SHA-256 matches a published
/// snapshot manifest header and the heavy root mtime is unchanged.
#[allow(clippy::too_many_arguments)]
pub fn try_lockfile_hit(
    store: &mut DiskStore,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: &ContentId,
    cfg: &RunConfig,
) -> SnapshotOutcome {
    SnapshotProjectionEngine::try_lockfile_hit(
        store,
        repo_root,
        pattern,
        src_root,
        heavy_rel,
        dest_root,
        lockfile_hash,
        cfg.verify,
    )
}

/// Build and hydrate one heavy directory through snapshots, or fall
/// back. `src_root` is where ingested content came from (for blob
/// healing), `dest_root` the new worktree root, `heavy_rel` the
/// repo-relative heavy directory name ("heavy" in `heavy/pkg/x`).
/// `repo_root` and `pattern` key the v2 selection index. `cfg.verify`
/// bypasses hits; v2 incremental rebuilds additionally require
/// `cfg.snapshots` and `cfg.v2`.
#[allow(clippy::too_many_arguments)]
pub fn hydrate(
    store: &mut DiskStore,
    ingested: &Ingested,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: Option<&ContentId>,
    cfg: &RunConfig,
) -> SnapshotOutcome {
    SnapshotProjectionEngine::hydrate(
        store,
        &ingested.dirs,
        &ingested.dir_modes,
        &ingested.files,
        &ingested.file_sizes,
        &ingested.symlinks,
        &ingested.modes,
        repo_root,
        pattern,
        src_root,
        heavy_rel,
        dest_root,
        lockfile_hash,
        cfg.verify,
        cfg.snapshots,
        cfg.v2,
    )
}
