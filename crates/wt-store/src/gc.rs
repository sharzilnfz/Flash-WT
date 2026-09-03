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
//! # Cutover Steps
//!
//! The cutover protocol moves the store from legacy per-blob refcounts
//! to store-local mirror marks in three verifiable stages:
//!
//! 1. Dual-write (`GcMode::Legacy`, default):
//!    All creation paths, including fallback materialization, tree snapshot
//!    hydration, and pinned lockfile snapshot hits, perform dual-writes.
//!    They publish store-local mirror records and maintain reference counts
//!    in `refs/` for all member blobs. Legacy sweepers check ref counts and
//!    execute `audit_marks_against_refs` to verify parity. Any divergence
//!    is reported as `wt-gc-audit:` diagnostic lines without collecting live data.
//!
//! 2. Activate mark-and-sweep (`wt store migrate --activate-mark-sweep`):
//!    Sets `gc-mode` to `mark-sweep`. Sweepers switch to live mirror marks
//!    and grace windows. Reference counts remain maintained on create and
//!    remove so older binaries or mixed-version runs remain safe.
//!
//! 3. Drop legacy refs (`wt store migrate --drop-legacy-refs`):
//!    Purges all `refs/` files and sets `gc-mode` to `mark-sweep-no-refs`.
//!    Creates and removes stop touching `refs/` entirely. This step is
//!    one-way; pre-cutover binaries must not use the store afterwards.
//!
//! Root validation is filesystem-existence based. A recorded worktree
//! path exists and is a directory, gitdir exists, and either the
//! `wt-hydrated.tsv` sidecar survives or the mirror is younger than
//! the grace period. No `git worktree list` calls. Git's
//! administrative records outlive `rm -rf` until pruned, so they are
//! not a liveness oracle.
//!
//! Unreferenced snapshots additionally face an LRU retention cap
//! (product-handoff §7.4). Past the grace filter at most
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
use crate::{ContentId, DiskStore, Error, Result};

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
        base_branch: Option<&str>,
        base_commit: Option<&str>,
    ) -> Result<PathBuf> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        let mut m = match self.read_worktree_mirror(&worktree, &gitdir)? {
            Some(existing) => existing,
            None => mirror::StoreMirror::new(worktree, gitdir),
        };
        for id in file_blobs {
            m.files.insert(*id);
        }
        for id in snapshots {
            m.snapshots.insert(*id);
        }
        if base_branch.is_some() {
            m.base_branch = base_branch.map(ToString::to_string);
        }
        if base_commit.is_some() {
            m.base_commit = base_commit.map(ToString::to_string);
        }
        mirror::publish(self.root(), &m).map_err(Error::Io)
    }

    /// Read the store mirror for a canonicalized (worktree, gitdir) identity.
    pub fn read_worktree_mirror(
        &self,
        worktree: &Path,
        gitdir: &Path,
    ) -> Result<Option<mirror::StoreMirror>> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        let path = mirror::mirror_path(self.root(), &worktree, &gitdir);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).map_err(Error::Io)?;
        let mirror = mirror::StoreMirror::parse(&text)
            .map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?;
        Ok(Some(mirror))
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
    /// Unlink the store mirror for a worktree if present.
    pub fn unlink_worktree_mirror(&self, worktree: &Path, gitdir: &Path) -> Result<bool> {
        let Ok(worktree) = fs::canonicalize(worktree) else {
            return Ok(false);
        };
        let Ok(gitdir) = fs::canonicalize(gitdir) else {
            return Ok(false);
        };
        mirror::remove(self.root(), &worktree, &gitdir).map_err(Error::Io)
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
        self.compute_marks_ext(now, grace, &BTreeSet::new(), false)
    }

    /// Compute live marks from store mirrors, optionally excluding specific mirror paths and dry-run flag.
    pub fn compute_marks_ext(
        &self,
        now: SystemTime,
        grace: Duration,
        ignored_mirrors: &BTreeSet<PathBuf>,
        dry_run: bool,
    ) -> Result<MarkReport> {
        let cutoff = cutoff_of(now, grace);
        let mut report = MarkReport::default();
        for read in mirror::read_all(self.root()) {
            let ReadMirror {
                path,
                modified,
                mirror,
            } = read;
            if ignored_mirrors.contains(&path) {
                report.dead_mirrors += 1;
                continue;
            }
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
                    if !dry_run {
                        let _ = fs::remove_file(&path);
                    }
                    report.stale_mirrors_removed.push(path);
                }
            }
        }
        Ok(report)
    }

    /// Sweep in a mirror-driven mode: delete unmarked blobs older
    /// than the grace period, unreferenced/debris snapshots older
    /// than it, snapshot temp data older than it, and stale mirrors.
    pub fn sweep_mark_sweep(&mut self, grace: Duration, snapshot_cap: usize) -> Result<MarkSwept> {
        self.sweep_mark_sweep_with_budget(grace, snapshot_cap, None)
    }

    /// Sweep with dual-budget snapshot limits (snapshot count cap plus max snapshot bytes).
    pub fn sweep_mark_sweep_with_budget(
        &mut self,
        grace: Duration,
        snapshot_cap: usize,
        max_snapshot_bytes: Option<u64>,
    ) -> Result<MarkSwept> {
        self.sweep_mark_sweep_with_policy(&SweepPolicy {
            grace,
            snapshot_cap,
            max_snapshot_bytes,
            dry_run: false,
        })
    }

    /// Sweep with a full SweepPolicy configuration.
    pub fn sweep_mark_sweep_with_policy(&mut self, policy: &SweepPolicy) -> Result<MarkSwept> {
        self.sweep_mark_sweep_with_policy_excluding(policy, &BTreeSet::new())
    }

    /// Sweep with a full SweepPolicy configuration and ignored mirrors.
    pub fn sweep_mark_sweep_with_policy_excluding(
        &mut self,
        policy: &SweepPolicy,
        ignored_mirrors: &BTreeSet<PathBuf>,
    ) -> Result<MarkSwept> {
        let now = SystemTime::now();
        let cutoff = cutoff_of(now, policy.grace);
        let marks = self.compute_marks_ext(now, policy.grace, ignored_mirrors, policy.dry_run)?;

        let ids = self.ids()?;
        let examined = ids.len() as u64;
        let mut outcome = MarkSwept {
            examined,
            reclaimed: 0,
            reclaimed_bytes: 0,
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
            let meta = match fs::metadata(self.object_path(&id)) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(Error::Io(e)),
            };
            let modified = match meta.modified() {
                Ok(m) => m,
                Err(e) => return Err(Error::Io(e)),
            };
            if modified > cutoff {
                continue;
            }
            if !policy.dry_run {
                self.delete(&id)?;
            }
            outcome.reclaimed += 1;
            outcome.reclaimed_bytes += meta.len();
        }

        let (swept, cap_evicted) = self.sweep_snapshots(
            &marks.referenced_snapshots,
            cutoff,
            policy.snapshot_cap,
            policy.max_snapshot_bytes,
            policy.dry_run,
        )?;
        outcome.snapshot_dirs_removed = swept + cap_evicted;
        outcome.snapshot_cap_evicted = cap_evicted;
        Ok(outcome)
    }

    /// Snapshot retention (dual-budget eviction: count cap + disk byte limits with anti-thrashing protection):
    /// snapshots are rebuildable caches, not roots. Unreferenced snapshots face dual budget limits
    /// using both count caps (`WT_SNAPSHOT_CAP`) and byte limits (`WT_MAX_SNAPSHOT_BYTES`).
    /// Snapshots younger than the grace period are protected against budget eviction to prevent cache thrashing.
    ///
    /// Returns `(grace-and-debris removals, retention-cap evictions)`.
    fn sweep_snapshots(
        &self,
        referenced: &BTreeSet<ContentId>,
        cutoff: SystemTime,
        cap: usize,
        max_bytes: Option<u64>,
        dry_run: bool,
    ) -> Result<(u64, u64)> {
        let mut removed = 0u64;
        let dir = self.root().join("snapshots");
        let Ok(entries) = fs::read_dir(&dir) else {
            return Ok((0, 0));
        };
        let _ = crate::snapindex::compact_journal(self.root());
        let lru = crate::snapindex::SnapshotLru::load(self.root());

        struct Candidate {
            path: PathBuf,
            last_use: u64,
            size: u64,
            is_young: bool,
        }

        let mut candidates: Vec<Candidate> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            match name.as_ref() {
                "index.tsv" | "lru.tsv" | "journal.tsv" | "metadata.lock" => {
                    // Live selection/LRU-retention metadata, not
                    // rebuildable cache: never collected.
                    continue;
                }
                "tmp" => {
                    // Temp build debris past the grace window.
                    if let Ok(tmp_entries) = fs::read_dir(&path) {
                        for tmp_entry in tmp_entries.flatten() {
                            let tmp_p = tmp_entry.path();
                            if let Ok(meta) = fs::metadata(&tmp_p) {
                                if let Ok(modified) = meta.modified() {
                                    if modified <= cutoff {
                                        if !dry_run {
                                            removed += u64::from(remove_tree(&tmp_p));
                                        } else {
                                            removed += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                _ => {}
            }
            let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let aged_out = modified <= cutoff;
            let id = ContentId::from_hex(&name);
            match id {
                Some(id) if referenced.contains(&id) => {}
                Some(id) => {
                    let size = if let Some(m) = crate::snapshot::read_published(self.root(), &id) {
                        if m.total_size > 0 {
                            m.total_size
                        } else {
                            let mut unique_blobs = BTreeSet::new();
                            let mut sum = 0u64;
                            for e in &m.entries {
                                if let Some(blob) = e.blob {
                                    if unique_blobs.insert(blob) {
                                        if let Ok(meta) = fs::metadata(self.blob_path(&blob)) {
                                            sum += meta.len();
                                        }
                                    }
                                }
                            }
                            sum
                        }
                    } else {
                        0
                    };
                    let last_use = lru.last_use(&id).unwrap_or_else(|| path_mtime_secs(&path));
                    candidates.push(Candidate {
                        path,
                        last_use,
                        size,
                        is_young: !aged_out,
                    });
                }
                None if aged_out => {
                    // Non-hex names are sidecar temp debris or tmp build dirs past the grace window.
                    if !dry_run {
                        removed += u64::from(remove_tree(&path));
                    } else {
                        removed += 1;
                    }
                }
                None => {}
            }
        }

        // Sort MRU first (highest last_use timestamp first)
        candidates.sort_by_key(|c| std::cmp::Reverse(c.last_use));

        let has_custom_budget = max_bytes.is_some() || cap < 64;

        let mut retained_count = 0usize;
        let mut retained_bytes = 0u64;
        let mut cap_evicted = 0u64;

        for c in candidates {
            if c.is_young {
                // Anti-thrashing grace window: recently published snapshots younger than the grace period
                // are protected from budget eviction even when budgets are tight.
                retained_count += 1;
                retained_bytes = retained_bytes.saturating_add(c.size);
                continue;
            }

            if !has_custom_budget {
                // Default sweep without custom budget: aged-out unreferenced snapshots are grace removals.
                if !dry_run {
                    if remove_tree(&c.path) {
                        removed += 1;
                    }
                } else {
                    removed += 1;
                }
                continue;
            }

            let count_ok = retained_count < cap;
            let bytes_ok = match max_bytes {
                Some(max) => retained_bytes.saturating_add(c.size) <= max,
                None => true,
            };

            if count_ok && bytes_ok {
                retained_count += 1;
                retained_bytes = retained_bytes.saturating_add(c.size);
            } else if !dry_run {
                if remove_tree(&c.path) {
                    cap_evicted += 1;
                }
            } else {
                cap_evicted += 1;
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
    /// Total bytes of unmarked, aged-out blobs deleted.
    pub reclaimed_bytes: u64,
    /// Stale mirrors removed.
    pub mirrors_removed: u64,
    /// Snapshot directories (including tmp debris) removed. Includes
    /// [`Self::snapshot_cap_evicted`].
    pub snapshot_dirs_removed: u64,
    /// Of those, how many went purely because the YOUNG unreferenced
    /// snapshot count exceeded the retention cap (LRU order), not
    /// because of age; aged-out unreferenced snapshots are always
    /// grace removals.
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

/// Filesystem work the CLI must perform after the store has unclaimed a lease.
/// The store releases refs, removes mirrors and lease files, then hands these
/// paths back for git and filesystem cleanup. The seam is unidirectional
/// (CLI calls store, never the reverse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCleanup {
    /// Worktree directory the CLI must remove.
    pub worktree: PathBuf,
    /// Administrative git dir the CLI must remove.
    pub gitdir: PathBuf,
    /// Branch the CLI must delete.
    pub branch: String,
}

/// Garbage collection and lease sweep policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPolicy {
    /// Grace duration before unreferenced objects and dead mirrors become eligible for reclamation.
    pub grace: Duration,
    /// Maximum number of unreferenced snapshots to retain past grace period.
    pub snapshot_cap: usize,
    /// Maximum disk byte budget for unreferenced snapshots.
    pub max_snapshot_bytes: Option<u64>,
    /// Whether to perform mark-and-sweep analysis without deleting files.
    pub dry_run: bool,
}

impl Default for SweepPolicy {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(15 * 60),
            snapshot_cap: 64,
            max_snapshot_bytes: None,
            dry_run: false,
        }
    }
}

/// Summary outcome of a full reclamation sweep pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepSummary {
    /// Active GC collection mode during the sweep.
    pub mode: GcMode,
    /// Total blob objects examined.
    pub examined_blobs: u64,
    /// Unreferenced, aged-out blobs deleted (or that would be deleted).
    pub reclaimed_blobs: u64,
    /// Total bytes of unreferenced blobs deleted (or that would be deleted).
    pub reclaimed_blob_bytes: u64,
    /// Stale or dead mirrors removed.
    pub mirrors_removed: u64,
    /// Snapshot directories (including temp debris) removed.
    pub snapshot_dirs_removed: u64,
    /// Snapshots evicted strictly to satisfy snapshot retention caps/budgets.
    pub snapshot_cap_evicted: u64,
    /// Whether deletion of unmarked objects was deferred due to young malformed mirrors.
    pub deferred_by_grace: bool,
    /// Total lease files examined.
    pub leases_examined: usize,
    /// Dead or expired leases successfully reclaimed.
    pub leases_reclaimed: usize,
    /// Estimated bytes of scratch worktrees reclaimed.
    pub lease_bytes_reclaimed: u64,
    /// Audit disagreement lines between mark-and-sweep and legacy refcounts.
    pub audit_disagreements: Vec<String>,
    /// Whether this sweep was a dry run.
    pub dry_run: bool,
}

/// Summary receipt returned when a worktree is retired.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetirementReceipt {
    /// Count of distinct blob references decremented.
    pub references_released: usize,
    /// Whether a store mirror was removed.
    pub mirror_removed: bool,
}

/// Unified store and lease reclamation engine. Owns metadata unclaim only:
/// ref releases, mirror removal, lease-file removal. Filesystem and git
/// lifecycle belongs to the CLI, which executes the returned cleanups.
pub struct StoreReclaimer<'a> {
    store: &'a mut DiskStore,
    dead_mirrors: BTreeSet<PathBuf>,
}

impl<'a> StoreReclaimer<'a> {
    /// Construct a new `StoreReclaimer` on the given store.
    pub fn new(store: &'a mut DiskStore) -> Self {
        Self {
            store,
            dead_mirrors: BTreeSet::new(),
        }
    }

    /// Sweep dead or expired worktree leases. Releases refs, removes mirrors
    /// and lease files, and returns the filesystem cleanups the caller must
    /// execute BEFORE calling [`Self::sweep_objects`]: the mark phase treats
    /// a worktree gone from disk as dead, so the caller's deletions must land
    /// between the two phases. `lease_bytes_reclaimed` is left to the caller,
    /// which measures worktree sizes before running the cleanups.
    pub fn sweep_leases(
        &mut self,
        policy: &SweepPolicy,
    ) -> Result<(usize, usize, Vec<PendingCleanup>)> {
        let now = SystemTime::now();
        let cutoff = cutoff_of(now, policy.grace);
        let leases = crate::lease::read_all(self.store.root());
        let leases_examined = leases.len();
        let mut leases_reclaimed = 0usize;
        let mut pending = Vec::new();

        for read in leases {
            let crate::lease::ReadLease {
                path,
                id,
                modified,
                lease,
            } = read;

            match lease {
                Ok(lease) => {
                    let is_dead = !crate::lease::is_process_alive(lease.pid, lease.start_time);
                    let is_expired = crate::lease::is_lease_expired(&lease);
                    let is_orphaned = !lease.worktree.exists();

                    if is_dead || is_expired || is_orphaned {
                        let mirror_p = crate::mirror::mirror_path(
                            self.store.root(),
                            &lease.worktree,
                            &lease.gitdir,
                        );
                        self.dead_mirrors.insert(mirror_p);

                        if !policy.dry_run {
                            if lease.gitdir.exists() {
                                let ledger_path = lease.gitdir.join("wt-hydrated.tsv");
                                if ledger_path.exists() {
                                    if let Ok(text) = fs::read_to_string(&ledger_path) {
                                        if self.store.gc_mode() != GcMode::MarkSweepNoRefs {
                                            let mut blob_ids = BTreeSet::new();
                                            let mut snapshot_ids = BTreeSet::new();
                                            for line in text.lines() {
                                                if line.is_empty() {
                                                    continue;
                                                }
                                                let fields: Vec<&str> = line.split('\t').collect();
                                                match fields.as_slice() {
                                                    [_, id_str] => {
                                                        if let Some(cid) =
                                                            ContentId::from_hex(id_str)
                                                        {
                                                            blob_ids.insert(cid);
                                                        }
                                                    }
                                                    [_, kind, id_str] if *kind == "blob" => {
                                                        if let Some(cid) =
                                                            ContentId::from_hex(id_str)
                                                        {
                                                            blob_ids.insert(cid);
                                                        }
                                                    }
                                                    [_, kind, id_str] if *kind == "snapshot" => {
                                                        if let Some(cid) =
                                                            ContentId::from_hex(id_str)
                                                        {
                                                            snapshot_ids.insert(cid);
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            for snap_id in &snapshot_ids {
                                                if let Some(manifest) =
                                                    crate::snapshot::read_published(
                                                        self.store.root(),
                                                        snap_id,
                                                    )
                                                {
                                                    for entry in &manifest.entries {
                                                        if let Some(blob) = entry.blob {
                                                            blob_ids.insert(blob);
                                                        }
                                                    }
                                                }
                                            }
                                            for cid in blob_ids {
                                                match self.store.release_ref(&cid) {
                                                    Ok(()) => {}
                                                    Err(Error::RefCountUnderflow(_)) => {}
                                                    Err(e) => return Err(e),
                                                }
                                            }
                                        }
                                    }
                                    let _ = fs::remove_file(&ledger_path);
                                }
                            }
                            let _ = self
                                .store
                                .remove_worktree_mirror(&lease.worktree, &lease.gitdir);

                            let _ = crate::lease::remove(self.store.root(), &id);
                            let _ = fs::remove_file(&path);
                        }

                        let branch_name = if lease.gitdir.exists() {
                            fs::read_to_string(lease.gitdir.join("HEAD"))
                                .ok()
                                .and_then(|h| {
                                    let h = h.trim();
                                    h.strip_prefix("ref: refs/heads/").map(|s| s.to_string())
                                })
                        } else {
                            None
                        };
                        let branch_name = branch_name.unwrap_or_else(|| {
                            if lease.id.starts_with("scratch-") {
                                lease.id.clone()
                            } else {
                                format!("scratch-{}", lease.id)
                            }
                        });

                        pending.push(PendingCleanup {
                            worktree: lease.worktree.clone(),
                            gitdir: lease.gitdir.clone(),
                            branch: branch_name,
                        });

                        leases_reclaimed += 1;
                    }
                }
                Err(_reason) => {
                    if modified <= cutoff {
                        if !policy.dry_run {
                            let _ = fs::remove_file(&path);
                        }
                        leases_reclaimed += 1;
                    }
                }
            }
        }

        Ok((leases_examined, leases_reclaimed, pending))
    }

    /// Sweep store objects under the given policy. Runs AFTER the caller has
    /// executed the [`Self::sweep_leases`] cleanups: the mark phase treats a
    /// worktree gone from disk as dead. Lease fields are zeroed; the caller
    /// fills them from its `sweep_leases` outcome plus measured byte sizes.
    pub fn sweep_objects(&mut self, policy: &SweepPolicy) -> Result<SweepSummary> {
        let mode = self.store.gc_mode();
        match mode {
            GcMode::Legacy => {
                let (swept, reclaimed_bytes) =
                    self.store.sweep_ext(policy.grace, policy.dry_run)?;
                let audit_disagreements = self.store.audit_marks_against_refs(policy.grace)?;
                Ok(SweepSummary {
                    mode,
                    examined_blobs: swept.examined,
                    reclaimed_blobs: swept.reclaimed,
                    reclaimed_blob_bytes: reclaimed_bytes,
                    mirrors_removed: 0,
                    snapshot_dirs_removed: 0,
                    snapshot_cap_evicted: 0,
                    deferred_by_grace: false,
                    leases_examined: 0,
                    leases_reclaimed: 0,
                    lease_bytes_reclaimed: 0,
                    audit_disagreements,
                    dry_run: policy.dry_run,
                })
            }
            GcMode::MarkSweep | GcMode::MarkSweepNoRefs => {
                let swept = self
                    .store
                    .sweep_mark_sweep_with_policy_excluding(policy, &self.dead_mirrors)?;
                Ok(SweepSummary {
                    mode,
                    examined_blobs: swept.examined,
                    reclaimed_blobs: swept.reclaimed,
                    reclaimed_blob_bytes: swept.reclaimed_bytes,
                    mirrors_removed: swept.mirrors_removed,
                    snapshot_dirs_removed: swept.snapshot_dirs_removed,
                    snapshot_cap_evicted: swept.snapshot_cap_evicted,
                    deferred_by_grace: swept.deferred_by_grace,
                    leases_examined: 0,
                    leases_reclaimed: 0,
                    lease_bytes_reclaimed: 0,
                    audit_disagreements: Vec::new(),
                    dry_run: policy.dry_run,
                })
            }
        }
    }

    /// Retire a worktree's store metadata: parse the sidecar ledger while the
    /// git dir still exists, hold blob ids in memory, release refs (tolerant
    /// of underflow), remove the ledger, and remove the store mirror. The
    /// caller owns git and filesystem removal afterwards.
    pub fn retire_worktree(
        &mut self,
        worktree_path: &Path,
        gitdir: &Path,
    ) -> Result<RetirementReceipt> {
        let ledger_path = gitdir.join("wt-hydrated.tsv");
        let mut blob_ids = BTreeSet::new();
        let mut snapshot_ids = BTreeSet::new();
        let mut had_ledger = false;

        if ledger_path.exists() {
            had_ledger = true;
            if let Ok(text) = fs::read_to_string(&ledger_path) {
                for line in text.lines() {
                    if line.is_empty() {
                        continue;
                    }
                    let fields: Vec<&str> = line.split('\t').collect();
                    match fields.as_slice() {
                        [_, id_str] => {
                            if let Some(cid) = ContentId::from_hex(id_str) {
                                blob_ids.insert(cid);
                            }
                        }
                        [_, kind, id_str] if *kind == "blob" => {
                            if let Some(cid) = ContentId::from_hex(id_str) {
                                blob_ids.insert(cid);
                            }
                        }
                        [_, kind, id_str] if *kind == "snapshot" => {
                            if let Some(cid) = ContentId::from_hex(id_str) {
                                snapshot_ids.insert(cid);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if (!blob_ids.is_empty() || !snapshot_ids.is_empty())
            && self
                .store
                .mirror_is_missing(worktree_path, gitdir)
                .unwrap_or(false)
        {
            let _ = self.store.publish_worktree_mirror(
                worktree_path,
                gitdir,
                blob_ids.iter(),
                snapshot_ids.iter(),
                None,
                None,
            );
        }

        let canon_worktree = fs::canonicalize(worktree_path).ok();
        let canon_gitdir = fs::canonicalize(gitdir).ok();

        if self.store.gc_mode() != GcMode::MarkSweepNoRefs {
            for snap_id in &snapshot_ids {
                if let Some(manifest) = crate::snapshot::read_published(self.store.root(), snap_id)
                {
                    for entry in &manifest.entries {
                        if let Some(blob) = entry.blob {
                            blob_ids.insert(blob);
                        }
                    }
                }
            }
        }

        let mut references_released = 0;
        if !blob_ids.is_empty() && self.store.gc_mode() != GcMode::MarkSweepNoRefs {
            for cid in &blob_ids {
                match self.store.release_ref(cid) {
                    Ok(()) => {
                        references_released += 1;
                    }
                    Err(Error::RefCountUnderflow(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }

        if had_ledger {
            let _ = fs::remove_file(&ledger_path);
        }

        let mirror_removed = if let (Some(cw), Some(cg)) = (canon_worktree, canon_gitdir) {
            crate::mirror::remove(self.store.root(), &cw, &cg).unwrap_or(false) || had_ledger
        } else {
            self.store
                .unlink_worktree_mirror(worktree_path, gitdir)
                .unwrap_or(false)
                || had_ledger
        };

        Ok(RetirementReceipt {
            references_released,
            mirror_removed,
        })
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_reclaimer_sweeps_dead_leases_and_unreferenced_blobs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_root = dir.path().join("store");
        let mut store = DiskStore::open(&store_root).expect("open store");

        let _id = store.put(b"hello world").expect("put blob");

        // Dead lease
        let lease = crate::lease::WorktreeLease::new(
            "scratch-1",
            dir.path().join("worktree-dead"),
            dir.path().join("gitdir-dead"),
            999_999_999,
            0,
            1900000000,
        );
        crate::lease::publish(&store_root, &lease).expect("publish lease");

        let mut reclaimer = StoreReclaimer::new(&mut store);
        let policy = SweepPolicy {
            grace: Duration::ZERO,
            snapshot_cap: 64,
            max_snapshot_bytes: None,
            dry_run: false,
        };

        let (leases_examined, leases_reclaimed, pending) =
            reclaimer.sweep_leases(&policy).expect("sweep leases");
        assert_eq!(leases_examined, 1);
        assert_eq!(leases_reclaimed, 1);
        assert_eq!(pending.len(), 1);
        let summary = reclaimer.sweep_objects(&policy).expect("sweep objects");
        assert_eq!(summary.reclaimed_blobs, 1);
        assert_eq!(summary.examined_blobs, 1);
    }

    #[test]
    fn store_reclaimer_dry_run_reports_without_deleting() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_root = dir.path().join("store");
        let mut store = DiskStore::open(&store_root).expect("open store");

        let id = store.put(b"hello unreferenced blob").expect("put blob");
        let blob_file = store.object_path(&id);
        assert!(blob_file.exists());

        // Dead lease
        let lease = crate::lease::WorktreeLease::new(
            "scratch-dry",
            dir.path().join("worktree-dry"),
            dir.path().join("gitdir-dry"),
            999_999_999,
            0,
            1900000000,
        );
        let lease_p = crate::lease::publish(&store_root, &lease).expect("publish lease");
        assert!(lease_p.exists());

        let usage_before = store.disk_usage().expect("disk usage");
        assert!(usage_before.objects_bytes > 0);
        assert!(usage_before.total_bytes > 0);

        let mut reclaimer = StoreReclaimer::new(&mut store);
        let policy = SweepPolicy {
            grace: Duration::ZERO,
            snapshot_cap: 64,
            max_snapshot_bytes: None,
            dry_run: true,
        };

        let (leases_examined, leases_reclaimed, pending) =
            reclaimer.sweep_leases(&policy).expect("sweep leases");
        assert_eq!(leases_examined, 1);
        assert_eq!(leases_reclaimed, 1);
        assert_eq!(pending.len(), 1);

        // In dry-run, lease file must not be deleted
        assert!(lease_p.exists());

        let summary = reclaimer.sweep_objects(&policy).expect("sweep objects");
        assert_eq!(summary.examined_blobs, 1);
        assert_eq!(summary.reclaimed_blobs, 1);
        assert!(summary.reclaimed_blob_bytes > 0);
        assert!(summary.dry_run);

        // In dry-run, blob must still exist on disk
        assert!(blob_file.exists());

        // Now run live sweep
        let mut reclaimer_live = StoreReclaimer::new(&mut store);
        let live_policy = SweepPolicy {
            grace: Duration::ZERO,
            snapshot_cap: 64,
            max_snapshot_bytes: None,
            dry_run: false,
        };

        let (_, live_reclaimed, _) = reclaimer_live
            .sweep_leases(&live_policy)
            .expect("live sweep leases");
        assert_eq!(live_reclaimed, 1);
        assert!(!lease_p.exists());

        let live_summary = reclaimer_live
            .sweep_objects(&live_policy)
            .expect("live sweep objects");
        assert_eq!(live_summary.reclaimed_blobs, 1);
        assert!(!blob_file.exists());
    }

    #[test]
    fn store_reclaimer_retire_worktree_releases_refs_and_removes_mirror() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_root = dir.path().join("store");
        let mut store = DiskStore::open(&store_root).expect("open store");

        let blob_id = store.put(b"worktree payload").expect("put blob");
        store.add_ref(&blob_id).expect("add ref");
        assert_eq!(store.ref_count(&blob_id).expect("ref count"), 1);

        let worktree = dir.path().join("my-wt");
        let gitdir = dir.path().join("my-gitdir");
        fs::create_dir_all(&worktree).expect("create wt");
        fs::create_dir_all(&gitdir).expect("create gitdir");

        let sidecar = gitdir.join("wt-hydrated.tsv");
        fs::write(&sidecar, format!("src/lib.rs\tblob\t{blob_id}\n")).expect("write sidecar");

        let mut blobs = BTreeSet::new();
        blobs.insert(blob_id);
        store
            .publish_worktree_mirror(
                &worktree,
                &gitdir,
                blobs.iter(),
                std::iter::empty(),
                None,
                None,
            )
            .expect("publish mirror");

        let mut reclaimer = StoreReclaimer::new(&mut store);
        let receipt = reclaimer
            .retire_worktree(&worktree, &gitdir)
            .expect("retire worktree");

        assert_eq!(receipt.references_released, 1);
        assert!(receipt.mirror_removed);
        assert_eq!(store.ref_count(&blob_id).expect("ref count"), 0);
        assert!(!sidecar.exists());
        assert!(
            worktree.exists(),
            "store retire leaves filesystem removal to the CLI"
        );
    }
}
