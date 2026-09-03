use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::mirror::{self, ReadMirror};
use crate::{ContentId, DiskStore, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcMode {
    Legacy,

    MarkSweep,

    MarkSweepNoRefs,
}

impl GcMode {
    pub const MARK_SWEEP: &'static str = "mark-sweep";

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

    pub fn read(root: &Path) -> GcMode {
        match fs::read_to_string(root.join("gc-mode")) {
            Ok(text) => GcMode::from_text(&text).unwrap_or(GcMode::Legacy),
            Err(_) => GcMode::Legacy,
        }
    }

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
    pub fn gc_mode(&self) -> GcMode {
        GcMode::read(self.root())
    }

    pub fn set_gc_mode(&self, mode: GcMode) -> io::Result<()> {
        mode.write(self.root())
    }

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

    pub fn remove_worktree_mirror(&self, worktree: &Path, gitdir: &Path) -> Result<()> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        mirror::remove(self.root(), &worktree, &gitdir)
            .map_err(Error::Io)
            .map(|_| ())
    }

    pub fn unlink_worktree_mirror(&self, worktree: &Path, gitdir: &Path) -> Result<bool> {
        let Ok(worktree) = fs::canonicalize(worktree) else {
            return Ok(false);
        };
        let Ok(gitdir) = fs::canonicalize(gitdir) else {
            return Ok(false);
        };
        mirror::remove(self.root(), &worktree, &gitdir).map_err(Error::Io)
    }

    pub fn mirror_is_missing(&self, worktree: &Path, gitdir: &Path) -> Result<bool> {
        let worktree =
            fs::canonicalize(worktree).map_err(|e| Error::Io(path_error("worktree", e)))?;
        let gitdir = fs::canonicalize(gitdir).map_err(|e| Error::Io(path_error("gitdir", e)))?;
        Ok(!mirror::mirror_path(self.root(), &worktree, &gitdir).exists())
    }

    pub fn compute_marks(&self, now: SystemTime, grace: Duration) -> Result<MarkReport> {
        self.compute_marks_ext(now, grace, &BTreeSet::new(), false)
    }

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
                    report.invalid.push((path, reason));
                    if young {
                        report.invalid_young += 1;
                    }
                    continue;
                }
            };
            let worktree_alive = m.worktree.is_dir();
            let gitdir_alive = m.gitdir.exists();
            let sidecar_alive = gitdir_alive && m.gitdir.join("flashwt-hydrated.tsv").exists();
            if worktree_alive && gitdir_alive && (sidecar_alive || young) {
                report.live_mirrors += 1;
                mark_through(&mut report, self.root(), &m);
            } else {
                report.dead_mirrors += 1;

                if young {
                    mark_through(&mut report, self.root(), &m);
                }
                if !worktree_alive && !young {
                    if !dry_run {
                        let _ = fs::remove_file(&path);
                    }
                    report.stale_mirrors_removed.push(path);
                }
            }
        }
        Ok(report)
    }

    pub fn sweep_mark_sweep(&mut self, grace: Duration, snapshot_cap: usize) -> Result<MarkSwept> {
        self.sweep_mark_sweep_with_budget(grace, snapshot_cap, None)
    }

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

    pub fn sweep_mark_sweep_with_policy(&mut self, policy: &SweepPolicy) -> Result<MarkSwept> {
        self.sweep_mark_sweep_with_policy_excluding(policy, &BTreeSet::new())
    }

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
                    continue;
                }
                "tmp" => {
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
                    if !dry_run {
                        removed += u64::from(remove_tree(&path));
                    } else {
                        removed += 1;
                    }
                }
                None => {}
            }
        }

        candidates.sort_by_key(|c| std::cmp::Reverse(c.last_use));

        let has_custom_budget = max_bytes.is_some() || cap < 64;

        let mut retained_count = 0usize;
        let mut retained_bytes = 0u64;
        let mut cap_evicted = 0u64;

        for c in candidates {
            if c.is_young {
                retained_count += 1;
                retained_bytes = retained_bytes.saturating_add(c.size);
                continue;
            }

            if !has_custom_budget {
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

    pub fn audit_marks_against_refs(&self, grace: Duration) -> Result<Vec<String>> {
        let now = SystemTime::now();
        let marks = self.compute_marks(now, grace)?;
        let mut lines = Vec::new();
        for (path, reason) in &marks.invalid {
            lines.push(format!(
                "flashwt-gc-audit: invalid mirror {} ({reason})",
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
                lines.push(format!("flashwt-gc-audit: marked-only {id}"));
            }
        }
        for id in refs_live.difference(&marks.marked) {
            lines.push(format!("flashwt-gc-audit: refcounted-only {id}"));
        }
        Ok(lines)
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkSwept {
    pub examined: u64,

    pub reclaimed: u64,

    pub reclaimed_bytes: u64,

    pub mirrors_removed: u64,

    pub snapshot_dirs_removed: u64,

    pub snapshot_cap_evicted: u64,

    pub deferred_by_grace: bool,
}

#[derive(Debug, Default)]
pub struct MarkReport {
    pub marked: BTreeSet<ContentId>,

    pub live_mirrors: usize,

    pub dead_mirrors: usize,

    pub invalid: Vec<(PathBuf, String)>,

    pub invalid_young: usize,

    pub referenced_snapshots: BTreeSet<ContentId>,

    pub unresolved_snapshots: usize,

    pub stale_mirrors_removed: Vec<PathBuf>,
}

fn cutoff_of(now: SystemTime, grace: Duration) -> SystemTime {
    now.checked_sub(grace).unwrap_or(SystemTime::UNIX_EPOCH)
}

fn path_mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mark_through(report: &mut MarkReport, root: &Path, m: &mirror::StoreMirror) {
    report.marked.extend(m.files.iter().copied());
    for snapshot in &m.snapshots {
        match crate::snapshot::read_published(root, snapshot) {
            Some(manifest) => {
                report.referenced_snapshots.insert(*snapshot);

                for entry in &manifest.entries {
                    if let Some(blob) = entry.blob {
                        report.marked.insert(blob);
                    }
                }
            }
            None => {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCleanup {
    pub worktree: PathBuf,

    pub gitdir: PathBuf,

    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPolicy {
    pub grace: Duration,

    pub snapshot_cap: usize,

    pub max_snapshot_bytes: Option<u64>,

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepSummary {
    pub mode: GcMode,

    pub examined_blobs: u64,

    pub reclaimed_blobs: u64,

    pub reclaimed_blob_bytes: u64,

    pub mirrors_removed: u64,

    pub snapshot_dirs_removed: u64,

    pub snapshot_cap_evicted: u64,

    pub deferred_by_grace: bool,

    pub leases_examined: usize,

    pub leases_reclaimed: usize,

    pub lease_bytes_reclaimed: u64,

    pub audit_disagreements: Vec<String>,

    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetirementReceipt {
    pub references_released: usize,

    pub mirror_removed: bool,
}

pub struct StoreReclaimer<'a> {
    store: &'a mut DiskStore,
    dead_mirrors: BTreeSet<PathBuf>,
}

impl<'a> StoreReclaimer<'a> {
    pub fn new(store: &'a mut DiskStore) -> Self {
        Self {
            store,
            dead_mirrors: BTreeSet::new(),
        }
    }

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
                                let ledger_path = lease.gitdir.join("flashwt-hydrated.tsv");
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

    pub fn retire_worktree(
        &mut self,
        worktree_path: &Path,
        gitdir: &Path,
    ) -> Result<RetirementReceipt> {
        let ledger_path = gitdir.join("flashwt-hydrated.tsv");
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

        assert!(lease_p.exists());

        let summary = reclaimer.sweep_objects(&policy).expect("sweep objects");
        assert_eq!(summary.examined_blobs, 1);
        assert_eq!(summary.reclaimed_blobs, 1);
        assert!(summary.reclaimed_blob_bytes > 0);
        assert!(summary.dry_run);

        assert!(blob_file.exists());

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

        let worktree = dir.path().join("my-worktree");
        let gitdir = dir.path().join("my-gitdir");
        fs::create_dir_all(&worktree).expect("create worktree");
        fs::create_dir_all(&gitdir).expect("create gitdir");

        let sidecar = gitdir.join("flashwt-hydrated.tsv");
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
