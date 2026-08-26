//! Mark-and-sweep garbage collection (fast-hydration ticket 07),
//! plus the store mode marker that gates the transition away from
//! refcount-driven collection (ticket 06).
//!
//! Three modes live in `<root>/gc-mode` (absent means legacy):
//!
//! - `legacy`: liveness comes from `refs/` refcounts only. Mirrors
//!   are still written (dual-write) and the CLI runs a mark-vs-refs
//!   audit beside every sweep for parity evidence.
//! - `mark-sweep`: liveness comes from live-mirror marks plus the
//!   grace period only. `refs/` files are still maintained by
//!   create/remove so pre-cutover binaries stay safe, but sweep
//!   ignores them.
//! - `mark-sweep-no-refs`: as above, and create/remove stop touching
//!   `refs/` entirely. Only reachable through an explicit operator
//!   command; legacy binaries must not use the store afterwards.
//!
//! Root validation is filesystem-existence based — recorded worktree
//! path exists and is a directory, gitdir exists, and either the
//! `wt-hydrated.tsv` sidecar survives or the mirror is younger than
//! the grace period. No `git worktree list` calls: git's
//! administrative records outlive `rm -rf` until pruned, so they are
//! not a liveness oracle.
//!
//! Unreferenced snapshots additionally face an LRU retention cap
//! (product-handoff §7.4): past the grace filter at most
//! `snapshot_cap` of them survive each sweep (`WT_SNAPSHOT_CAP`,
//! default 64), least-recently-used first, with last-use stamps in
//! the [`crate::snapindex::SnapshotLru`] sidecar beside
//! `snapshots/`. Referenced snapshots never count against the cap.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::mirror::{self, ReadMirror};
use crate::{ContentId, DiskStore, Error, Result, Store};

/// The store's collection mode, persisted as `<root>/gc-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    /// Refcount-driven collection (ticket 06 behavior). Default.
    Legacy,
    /// Mirror-driven collection; refs maintained but ignored.
    MarkSweep,
    /// Mirror-driven collection; refs no longer written.
    MarkSweepNoRefs,
}

impl GcMode {
    /// Canonical name of the mark-and-sweep mode (ADR-0004).
    pub const MARK_SWEEP: &'static str = "mark-sweep";
    /// Canonical name of the refs-free mark-and-sweep variant.
    pub const MARK_SWEEP_NO_REFS: &'static str = "mark-sweep-no-refs";

    fn text(self) -> &'static str {
        match self {
            GcMode::Legacy => "",
            GcMode::MarkSweep => Self::MARK_SWEEP,
            GcMode::MarkSweepNoRefs => Self::MARK_SWEEP_NO_REFS,
        }
    }

    fn from_text(text: &str) -> Option<GcMode> {
        match text.trim() {
            Self::MARK_SWEEP => Some(GcMode::MarkSweep),
            Self::MARK_SWEEP_NO_REFS => Some(GcMode::MarkSweepNoRefs),
            _ => None,
        }
    }

    /// Read `<root>/gc-mode`; absent or unrecognized means legacy.
    pub fn read(root: &Path) -> GcMode {
        match fs::read_to_string(root.join("gc-mode")) {
            Ok(text) => GcMode::from_text(&text).unwrap_or(GcMode::Legacy),
            Err(_) => GcMode::Legacy,
        }
    }

    /// Write `<root>/gc-mode`. Writing legacy removes the marker,
    /// restoring pre-marker semantics for any binary. The write is
    /// crash-durable (fsync before rename): the mode marker gates
    /// collection strategy, so a half-landed marker is exactly the
    /// ambiguity the durability work exists to eliminate.
    pub fn write(self, root: &Path) -> io::Result<()> {
        let path = root.join("gc-mode");
        match self.text() {
            "" => match fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
            text => crate::fsutil::durable_write(&path, format!("{text}\n").as_bytes()),
        }
    }
}

impl DiskStore {
    /// The collection mode this store currently runs in.
    pub fn gc_mode(&self) -> GcMode {
        GcMode::read(self.root())
    }

    /// Switch the collection mode (one-way by policy; see ADR-0004).
    pub fn set_gc_mode(&self, mode: GcMode) -> io::Result<()> {
        mode.write(self.root())
    }

    /// Publish the authoritative mirror for one hydrated worktree.
    /// Both paths are canonicalized here, at write time, exactly as
    /// the key derivation requires. One atomic write per successful
    /// create — this replaces per-blob ref writes as GC bookkeeping.
    ///
    /// `file_blobs` carries one `file` record per per-file-placed
    /// blob; `snapshots` carries one `snapshot` record per heavy
    /// directory hydrated from a whole-directory snapshot. A snapshot
    /// hydration writes ONLY its snapshot record (the manifest marks
    /// every child blob); a fallback hydration writes file records.
    pub fn publish_worktree_mirror<'a>(
        &self,
        worktree: &Path,
        gitdir: &Path,
        file_blobs: impl IntoIterator<Item = &'a ContentId>,
        snapshots: impl IntoIterator<Item = &'a ContentId>,
    ) -> Result<PathBuf> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        let mut m = mirror::StoreMirror::new(worktree, gitdir);
        for id in file_blobs {
            m.files.insert(*id);
        }
        for id in snapshots {
            m.snapshots.insert(*id);
        }
        mirror::publish(self.root(), &m).map_err(Error::Io)
    }

    /// Remove the mirror of a removed worktree. Missing is fine: an
    /// interrupted remove must stay rerunnable.
    pub fn remove_worktree_mirror(&self, worktree: &Path, gitdir: &Path) -> Result<()> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        mirror::remove(self.root(), &worktree, &gitdir)
            .map_err(Error::Io)
            .map(|_| ())
    }

    /// True when the mirror for this identity is missing but the
    /// sidecar still exists — the state the plan says the next
    /// create/remove repairs by rewriting the mirror.
    pub fn mirror_is_missing(&self, worktree: &Path, gitdir: &Path) -> Result<bool> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        Ok(!mirror::mirror_path(self.root(), &worktree, &gitdir).exists())
    }

    /// Compute the live-mark set from store mirrors (the "mark"
    /// phase). Blobs named by `file` records of live mirrors are
    /// marked; `snapshot` records mark through their manifest when it
    /// resolves, and through nothing when it does not — an
    /// unresolvable snapshot never blocks collection (its worktree
    /// holds private clones and rebuilds on the next create).
    ///
    /// Stale mirrors whose recorded worktree path is gone and whose
    /// age exceeds the grace period are deleted here (sweep rule 5).
    ///
    /// A mirror inside the grace window marks through its records
    /// even when root validation fails: a worktree deleted seconds
    /// ago, or mid-move, must not have its blobs collected before the
    /// grace period expires. Only an aged-out failed root stops
    /// protecting content.
    pub fn compute_marks(&self, now: SystemTime, grace: Duration) -> Result<MarkReport> {
        let cutoff = cutoff_of(now, grace);
        let mut report = MarkReport::default();
        for read in mirror::read_all(self.root()) {
            let ReadMirror {
                path,
                modified,
                mirror,
            } = read;
            let young = modified > cutoff;
            let m = match mirror {
                Ok(m) => m,
                Err(reason) => {
                    // Malformed mirrors are preserved for diagnosis.
                    // Within the grace window they are never treated
                    // as empty for deletion decisions.
                    report.invalid.push((path, reason));
                    if young {
                        report.invalid_young += 1;
                    }
                    continue;
                }
            };
            let wt_alive = m.worktree.is_dir();
            let gitdir_alive = m.gitdir.exists();
            let sidecar_alive = gitdir_alive && m.gitdir.join("wt-hydrated.tsv").exists();
            if wt_alive && gitdir_alive && (sidecar_alive || young) {
                report.live_mirrors += 1;
                mark_through(&mut report, self.root(), &m);
            } else {
                report.dead_mirrors += 1;
                // A mirror inside the grace window keeps protecting
                // its records even though root validation failed:
                // the worktree may have just been moved, or died
                // seconds ago. Waiting one more grace period costs
                // disk; collecting early could cost a live tree.
                if young {
                    mark_through(&mut report, self.root(), &m);
                }
                if !wt_alive && !young {
                    let _ = fs::remove_file(&path);
                    report.stale_mirrors_removed.push(path);
                }
            }
        }
        Ok(report)
    }

    /// Sweep in a mirror-driven mode: delete unmarked blobs older
    /// than the grace period, unreferenced/debris snapshots older
    /// than it, snapshot temp data older than it, and stale mirrors.
    /// On top of the grace period, unreferenced snapshots are also
    /// capped at `snapshot_cap` entries (`WT_SNAPSHOT_CAP`, default
    /// 64): when more survive the grace filter than the cap allows,
    /// the least-recently-used go first, per product-handoff §7.4.
    ///
    /// If any malformed mirror is younger than the grace window, this
    /// pass deletes NOTHING beyond what [`DiskStore::compute_marks`]
    /// already removed: an unparsable mirror might name live roots we
    /// cannot see, and waiting one more grace period costs disk, not
    /// correctness.
    pub fn sweep_mark_sweep(&mut self, grace: Duration, snapshot_cap: usize) -> Result<MarkSwept> {
        let now = SystemTime::now();
        let cutoff = cutoff_of(now, grace);
        let marks = self.compute_marks(now, grace)?;

        let ids = self.ids()?;
        let examined = ids.len() as u64;
        let mut outcome = MarkSwept {
            examined,
            reclaimed: 0,
            mirrors_removed: marks.stale_mirrors_removed.len() as u64,
            snapshot_dirs_removed: 0,
            snapshot_cap_evicted: 0,
            deferred_by_grace: marks.invalid_young > 0,
        };
        if outcome.deferred_by_grace {
            return Ok(outcome);
        }

        for id in ids {
            if marks.marked.contains(&id) {
                continue;
            }
            let modified = fs::metadata(self.object_path(&id))
                .map_err(Error::Io)?
                .modified()
                .map_err(Error::Io)?;
            if modified > cutoff {
                continue;
            }
            self.delete(&id)?;
            outcome.reclaimed += 1;
        }

        let (swept, cap_evicted) =
            self.sweep_snapshots(&marks.referenced_snapshots, cutoff, snapshot_cap)?;
        outcome.snapshot_dirs_removed = swept + cap_evicted;
        outcome.snapshot_cap_evicted = cap_evicted;
        Ok(outcome)
    }

    /// Snapshot retention (plan's eviction rule + §7.4 LRU cap):
    /// snapshots are rebuildable caches, not roots. Anything under
    /// `snapshots/` that is either unreferenced by a live mirror or
    /// debris, and older than the cutoff, goes — and when more than
    /// `cap` unreferenced aged-out snapshots remain, the surplus is
    /// deleted least-recently-used first (LRU sidecar stamp, falling
    /// back to directory mtime; unknown age counts as oldest).
    /// Referenced snapshots never count against the cap, and young
    /// (inside-grace) ones are simply skipped this pass, whatever the
    /// cap says. Phase 1 stores have no snapshots directory, so this
    /// is normally a no-op scan.
    ///
    /// Returns `(grace-and-debris removals, retention-cap evictions)`.
    fn sweep_snapshots(
        &self,
        referenced: &BTreeSet<ContentId>,
        cutoff: SystemTime,
        cap: usize,
    ) -> Result<(u64, u64)> {
        let mut removed = 0u64;
        let dir = self.root().join("snapshots");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok((0, 0));
        };
        let lru = crate::snapindex::SnapshotLru::load(self.root());
        // Aged-out, unreferenced survivors of this pass, awaiting the
        // retention-cap decision.
        let mut candidates: Vec<(ContentId, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if modified > cutoff {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            match name.as_ref() {
                "tmp" => {
                    // Temp build debris past the grace window.
                    if remove_tree(&path) {
                        removed += 1;
                    }
                    continue;
                }
                "index.tsv" | "lru.tsv" => {
                    // Live selection/LRU-retention metadata, not
                    // rebuildable cache: never collected. (Their own
                    // temp-file debris has a non-hex name and falls
                    // through to collection below.)
                    continue;
                }
                _ => {}
            }
            let id = ContentId::from_hex(&name);
            match id {
                Some(id) if referenced.contains(&id) => {}
                Some(id) => candidates.push((id, path)),
                None => {
                    // Referenced-but-unresolvable snapshots stay:
                    // their blobs' fate follows blob marks alone, and the
                    // directory may still be mid-publish. Non-hex names
                    // are sidecar temp debris: collectible.
                    if remove_tree(&path) {
                        removed += 1;
                    }
                }
            }
        }

        let mut cap_evicted = 0u64;
        if candidates.len() > cap {
            // Least-recently-used first; the sort key falls back to
            // the publish-time directory mtime and finally to 0 for
            // anything unreadable, so unknown age loses.
            candidates.sort_by_key(|(id, path)| {
                lru.last_use(id).unwrap_or_else(|| path_mtime_secs(path))
            });
            let excess = candidates.len() - cap;
            for (_, path) in candidates.drain(..excess) {
                if remove_tree(&path) {
                    cap_evicted += 1;
                }
            }
        }
        Ok((removed, cap_evicted))
    }

    /// Audit mode (legacy sweeps): compare mirror-marks against
    /// refcount-derived liveness over the blobs actually present.
    /// Returns human-readable disagreement lines; empty means the two
    /// schemes agree, which is the parity evidence the transition
    /// relies on. Invalid mirrors are reported too.
    pub fn audit_marks_against_refs(&self, grace: Duration) -> Result<Vec<String>> {
        let now = SystemTime::now();
        let marks = self.compute_marks(now, grace)?;
        let mut lines = Vec::new();
        for (path, reason) in &marks.invalid {
            lines.push(format!(
                "wt-gc-audit: invalid mirror {} ({reason})",
                path.display()
            ));
        }
        let present: BTreeSet<ContentId> = self.ids()?.into_iter().collect();
        let mut refs_live = BTreeSet::new();
        for id in &present {
            if self.ref_count(id).unwrap_or(0) > 0 {
                refs_live.insert(*id);
            }
        }
        for id in marks.marked.intersection(&present) {
            if !refs_live.contains(id) {
                lines.push(format!("wt-gc-audit: marked-only {id}"));
            }
        }
        for id in refs_live.difference(&marks.marked) {
            lines.push(format!("wt-gc-audit: refcounted-only {id}"));
        }
        Ok(lines)
    }

    /// Delete every legacy `refs/<hex>` refcount file. Only ever
    /// called from the explicit drop-legacy-refs migration.
    pub fn purge_legacy_refs(&mut self) -> Result<usize> {
        let mut purged = 0usize;
        let entries = fs::read_dir(self.root().join("refs")).map_err(Error::Io)?;
        for entry in entries.flatten() {
            if entry.file_type().map_err(Error::Io)?.is_file() {
                fs::remove_file(entry.path()).map_err(Error::Io)?;
                purged += 1;
            }
        }
        Ok(purged)
    }
}

/// What one mark-and-sweep pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkSwept {
    /// Blob entries the pass looked at.
    pub examined: u64,
    /// Unmarked, aged-out blobs deleted.
    pub reclaimed: u64,
    /// Stale mirrors removed.
    pub mirrors_removed: u64,
    /// Snapshot directories (including tmp debris) removed. Includes
    /// [`Self::snapshot_cap_evicted`].
    pub snapshot_dirs_removed: u64,
    /// Of those, how many went purely because the unreferenced
    /// snapshot count exceeded the retention cap (LRU order), not
    /// because of age.
    pub snapshot_cap_evicted: u64,
    /// True when a malformed mirror inside the grace window deferred
    /// all deletion this pass.
    pub deferred_by_grace: bool,
}

/// Marks plus everything observed while computing them.
#[derive(Debug, Default)]
pub struct MarkReport {
    /// Content marked live through valid mirrors.
    pub marked: BTreeSet<ContentId>,
    /// Mirrors whose marks were all applied.
    pub live_mirrors: usize,
    /// Mirrors skipped as dead or expired past the grace window.
    pub dead_mirrors: usize,
    /// (path, reason) for mirrors that failed to parse; preserved on
    /// disk for diagnosis.
    pub invalid: Vec<(PathBuf, String)>,
    /// How many of those are younger than the grace window.
    pub invalid_young: usize,
    /// Snapshot records that resolved to a published manifest.
    pub referenced_snapshots: BTreeSet<ContentId>,
    /// Snapshot records pointing at missing/incomplete manifests.
    pub unresolved_snapshots: usize,
    /// Mirrors deleted because their worktree is gone and they aged
    /// past the grace period.
    pub stale_mirrors_removed: Vec<PathBuf>,
}

fn cutoff_of(now: SystemTime, grace: Duration) -> SystemTime {
    now.checked_sub(grace).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Unix seconds of `path`'s mtime, or 0 when it cannot be read — the
/// LRU eviction order treats unknown age as oldest.
fn path_mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Apply one parsed mirror's records to the report.
fn mark_through(report: &mut MarkReport, root: &Path, m: &mirror::StoreMirror) {
    report.marked.extend(m.files.iter().copied());
    for snapshot in &m.snapshots {
        match crate::snapshot::read_published(root, snapshot) {
            Some(manifest) => {
                report.referenced_snapshots.insert(*snapshot);
                // Mark every file entry's blob: the snapshot is only
                // alive while referenced, and its blobs must outlive
                // it. Symlinks and dirs reference no blobs.
                for entry in &manifest.entries {
                    if let Some(blob) = entry.blob {
                        report.marked.insert(blob);
                    }
                }
            }
            None => {
                // A missing, invalid, or incomplete snapshot marks
                // through nothing — its worktree holds private clones
                // and rebuilds on the next create.
                report.unresolved_snapshots += 1;
            }
        }
    }
}

fn remove_tree(path: &Path) -> bool {
    if path.is_dir() {
        fs::remove_dir_all(path).is_ok()
    } else {
        fs::remove_file(path).is_ok()
    }
}

fn path_error(what: &str, e: io::Error) -> io::Error {
    io::Error::other(format!("cannot canonicalize {what}: {e}"))
}
