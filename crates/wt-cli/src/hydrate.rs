//! Store-backed hydration (ticket 05).
//!
//! Heavy directories no longer copy straight from the source checkout.
//! They are ingested into the content-addressed store (every unique
//! file once, addressed by hash), then materialized into the new
//! worktree from the store. A second worktree of the same project
//! therefore adds no new store content — the whole point of ADR-0001.
//!
//! References: every blob a worktree materializes gets one `add_ref`,
//! attributed to that worktree through a sidecar manifest written into
//! the worktree's git dir (`wt-hydrated.tsv`). Ticket 06's garbage
//! collection releases those references when the worktree dies.
//!
//! Integrity: materialize proves each blob before anything is
//! placed, so corrupt store content fails loudly instead of landing
//! bad bytes in a fresh tree (spec: silent corruption is detectable).
//! Fast-hydration ticket 05 makes that proof cost once per blob
//! rather than once per run: `ensure_verified` re-hashes only blobs
//! whose (size, mtime) fingerprint is not already recorded in the
//! verified ledger beside the store. `WT_VERIFY=1` restores the old
//! full-hash-every-blob-every-run paranoia.
//!
//! Materialization strategy (fast-hydration ticket 03): the default
//! places every blob as a per-file copy-on-write clone (`fclonefileat`
//! on macOS) — a fresh private inode sharing the blob's physical
//! blocks until first write, with normal writable permissions.
//! Filesystems that refuse the clone fall back silently to byte
//! copies; Linux gets byte copies until reflink is validated there.
//! Hardlinked materialization survives behind `WT_HARDLINK=1` as an
//! experimental mode (see below), and `WT_NO_HARDLINK=1` still forces
//! plain byte copies.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Instant;

use wt_copy::Materializer;
#[cfg(target_os = "macos")]
use wt_store::bulkwalk;
use wt_store::{ContentId, DiskStore, Entry as CacheEntry, GcMode, Store, ValidationCache};

use crate::config::{RunConfig, StrategyPolicy};
use crate::envelope::Diagnostic;
use crate::error::{Error, Result};
use crate::gitops;
use crate::manifest::{collect_matches, pattern_matches};
use crate::snapshots;
use crate::snapshots::Outcome as SnapshotOutcome;
use crate::timing::StageTimings;

/// Where the per-machine store lives. `$WT_STORE` wins (tests use it
/// for isolation); otherwise XDG cache conventions.
fn store_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("WT_STORE") {
        return Ok(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => {
            let home = std::env::var_os("HOME").ok_or_else(|| {
                Error::Usage("cannot locate a home directory for the store".into())
            })?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("wt").join("store"))
}

pub fn open_store() -> Result<DiskStore> {
    let dir = store_dir()?;
    DiskStore::open(&dir)
        .map_err(|e| Error::Store(format!("cannot open store at {}: {e}", dir.display())))
}

/// Request parameters for worktree hydration.
pub struct HydrationRequest<'a> {
    pub root: &'a Path,
    pub dest: &'a Path,
    pub patterns: &'a [String],
    pub base_branch: Option<&'a str>,
    pub base_commit: Option<&'a str>,
    pub cfg: &'a RunConfig,
}

/// Consolidated report of worktree hydration operations and metrics.
#[allow(dead_code)]
pub struct HydrationReport {
    pub total_files: usize,
    pub total_copied: usize,
    pub bytes_shared_cow: u64,
    pub bytes_copied: u64,
    pub strategy: &'static str,
    pub hydration_method: String,
    pub cache_hit: bool,
    pub snapshot_hashes: Vec<ContentId>,
    pub dirs_hydrated: Vec<PathBuf>,
    pub timings: StageTimings,
    pub diagnostics: Vec<Diagnostic>,
}

/// Deep hydration engine coordinating discovery, caching, snapshot
/// projection, parallel materialization, mirror publishing, and ledger persistence.
pub struct HydrationEngine<'a> {
    store: &'a mut DiskStore,
}

impl<'a> HydrationEngine<'a> {
    /// Create a new `HydrationEngine` backed by the given disk store.
    pub fn new(store: &'a mut DiskStore) -> Self {
        Self { store }
    }

    /// Execute the complete hydration pipeline for the given request.
    pub fn hydrate(&mut self, req: HydrationRequest<'_>) -> Result<HydrationReport> {
        let mut timings = StageTimings::new();
        let dirs = collect_matches(req.root, req.patterns)?;

        if dirs.is_empty() {
            let git_dir = gitops::git_dir(req.dest)?;
            let combined = Ingested {
                dirs: Vec::new(),
                dir_modes: BTreeMap::new(),
                files: BTreeMap::new(),
                file_sizes: BTreeMap::new(),
                symlinks: BTreeMap::new(),
                modes: BTreeMap::new(),
            };
            publish_mirror(
                self.store,
                req.dest,
                &git_dir,
                &combined,
                &[],
                req.base_branch,
                req.base_commit,
            )?;

            let diagnostics =
                crate::base::check_base_movement(self.store, req.root, req.base_branch);
            for d in &diagnostics {
                if !req.cfg.json {
                    eprintln!("wt: warning: {}", d.message);
                }
            }

            if !req.cfg.json {
                println!("nothing to hydrate");
            }

            return Ok(HydrationReport {
                total_files: 0,
                total_copied: 0,
                bytes_shared_cow: 0,
                bytes_copied: 0,
                strategy: "none",
                hydration_method: "none".to_string(),
                cache_hit: false,
                snapshot_hashes: Vec::new(),
                dirs_hydrated: Vec::new(),
                timings,
                diagnostics,
            });
        }

        let mut total_files = 0usize;
        let mut total_copied = 0usize;
        let mut bytes_shared_cow = 0u64;
        let mut bytes_copied = 0u64;
        let mut strategy = "byte-copy";
        let mut combined = Ingested {
            dirs: Vec::new(),
            dir_modes: BTreeMap::new(),
            files: BTreeMap::new(),
            file_sizes: BTreeMap::new(),
            symlinks: BTreeMap::new(),
            modes: BTreeMap::new(),
        };
        let mut snapshot_hashes: Vec<ContentId> = Vec::new();
        let mut git_dir = req.dest.to_path_buf();
        let mut dirs_hydrated = Vec::new();

        for rel in &dirs {
            dirs_hydrated.push(rel.clone());
            let outcome =
                self.hydrate_one_dir(req.patterns, req.root, req.dest, rel, req.cfg, &mut timings)?;
            git_dir = outcome.git_dir;
            if let Some(hash) = outcome.snapshot_hash {
                snapshot_hashes.push(hash);
            }
            match outcome.ladder {
                None => {
                    total_files += outcome.ingested.files.len();
                    bytes_shared_cow += outcome.ingested.file_sizes.values().sum::<u64>();
                }
                Some(report) => {
                    combined.dirs.extend(outcome.ingested.dirs.iter().cloned());
                    for (r, mode) in &outcome.ingested.dir_modes {
                        combined.dir_modes.insert(r.clone(), *mode);
                    }
                    for (r, id) in &outcome.ingested.files {
                        combined.files.insert(r.clone(), *id);
                    }
                    for (r, size) in &outcome.ingested.file_sizes {
                        combined.file_sizes.insert(r.clone(), *size);
                    }
                    total_files += report.files;
                    total_copied += report.copied;
                    bytes_shared_cow += report.bytes_shared;
                    bytes_copied += report.bytes_copied;
                    strategy = report.strategy;
                }
            }
        }

        let stage = Instant::now();
        publish_mirror(
            self.store,
            req.dest,
            &git_dir,
            &combined,
            &snapshot_hashes,
            req.base_branch,
            req.base_commit,
        )?;
        timings.references_ms += stage.elapsed().as_millis();

        crate::toolchain::relocate_toolchains(req.root, req.dest, &dirs)?;

        if !req.cfg.json {
            if req.cfg.strategy_policy == StrategyPolicy::ForceByteCopy {
                println!(
                    "hardlink mode off (WT_NO_HARDLINK): wrote byte copies for all {total_files} file(s)"
                );
            } else {
                match (strategy, total_copied) {
                    ("hardlink", 0) => println!(
                        "experimental hardlink mode (WT_HARDLINK): linked shared inodes for all {total_files} file(s)"
                    ),
                    ("hardlink", n) => println!(
                        "experimental hardlink mode (WT_HARDLINK): hardlinks refused for {n} of {total_files} file(s); wrote byte copies"
                    ),
                    (_, 0) => {}
                    (name, n) => println!(
                        "{name} unavailable on this filesystem: wrote byte copies for {n} of {total_files} file(s)"
                    ),
                }
            }
        }

        self.store.flush().map_err(|e| {
            Error::io_unanchored("update verified-blob ledger", self.store.root(), e)
        })?;

        if !req.cfg.json {
            println!(
                "hydration complete: {total_files} file{} through the store",
                if total_files == 1 { "" } else { "s" }
            );
        }

        let mut diagnostics =
            crate::base::check_base_movement(self.store, req.root, req.base_branch);
        if total_copied > 0 {
            diagnostics.push(Diagnostic::warning(
                "CROSS_DEVICE_COPY_DEGRADATION",
                format!(
                    "Storage boundaries or filesystem refusal forced fallback byte copies for {total_copied} of {total_files} file(s)"
                ),
            ));
        }
        for d in &diagnostics {
            if !req.cfg.json {
                eprintln!("wt: warning: {}", d.message);
            }
        }

        let hydration_method = if total_files == 0 {
            "none"
        } else if snapshot_hashes.len() == dirs.len()
            || (strategy == "copy-on-write" && total_copied == 0)
        {
            "clone"
        } else if strategy == "hardlink" && total_copied == 0 {
            "hardlink"
        } else if total_copied == total_files {
            "byte_copy"
        } else {
            match strategy {
                "copy-on-write" => "clone",
                "reflink" => "reflink",
                "copy_file_range" => "copy_file_range",
                "hardlink" => "hardlink",
                _ => "byte_copy",
            }
        }
        .to_string();

        let cache_hit = (snapshot_hashes.len() == dirs.len() && !timings.snapshot_built)
            || (total_files > 0 && total_copied == 0 && !timings.snapshot_built);

        Ok(HydrationReport {
            total_files,
            total_copied,
            bytes_shared_cow,
            bytes_copied,
            strategy,
            hydration_method,
            cache_hit,
            snapshot_hashes,
            dirs_hydrated,
            timings,
            diagnostics,
        })
    }

    fn hydrate_one_dir(
        &mut self,
        patterns: &[String],
        root: &Path,
        dest: &Path,
        rel: &Path,
        cfg: &RunConfig,
        timings: &mut StageTimings,
    ) -> Result<DirOutcome> {
        let src = root.join(rel);
        let heavy = rel.to_string_lossy().into_owned();

        let pattern = patterns
            .iter()
            .find(|p| pattern_matches(p, rel))
            .map(String::as_str)
            .unwrap_or("");

        let lockfile_info = wt_store::find_lockfile(root, rel).and_then(|lp| {
            let content = std::fs::read(&lp).ok()?;
            let text = std::str::from_utf8(&content).ok()?;
            let safety = wt_store::classify_lockfile(text);
            let hash = wt_store::hash_lockfile(&content);
            Some((safety, hash))
        });

        let pinned_lockfile_hash = match lockfile_info {
            Some((wt_store::DependencySafety::Pinned, hash)) => Some(hash),
            _ => None,
        };

        if cfg.snapshots && !cfg.verify {
            if let Some(ref lock_hash) = pinned_lockfile_hash {
                let stage = Instant::now();
                match snapshots::try_lockfile_hit(
                    self.store, root, pattern, root, &heavy, dest, lock_hash, cfg,
                ) {
                    SnapshotOutcome::Hydrated(h) => {
                        timings.snapshot_ms += stage.elapsed().as_millis();
                        timings.snapshot_engaged = true;
                        timings.snapshot_lookup_ms += h.lookup_ms;
                        timings.snapshot_clonefile_ms += h.clonefile_ms;
                        timings.snapshot_mode = h.mode;
                        timings.v2_cloned += h.cloned_units;
                        timings.v2_linked += h.linked_files;
                        let refs = Instant::now();
                        let empty_ingested = Ingested {
                            dirs: Vec::new(),
                            dir_modes: BTreeMap::new(),
                            files: BTreeMap::new(),
                            file_sizes: BTreeMap::new(),
                            symlinks: BTreeMap::new(),
                            modes: BTreeMap::new(),
                        };
                        let git_dir =
                            claim_snapshot_references(self.store, dest, &empty_ingested, h.hash)?;
                        timings.references_ms += refs.elapsed().as_millis();
                        if !cfg.json {
                            println!(
                                "hydrated {heavy} from {} via snapshot {} (one clone, {} file{})",
                                src.display(),
                                &h.hash.to_string()[..12],
                                h.files,
                                if h.files == 1 { "" } else { "s" },
                            );
                        }
                        return Ok(DirOutcome {
                            git_dir,
                            snapshot_hash: Some(h.hash),
                            ingested: empty_ingested,
                            ladder: None,
                        });
                    }
                    SnapshotOutcome::FellBack(Some(reason)) => {
                        eprintln!("wt-snapshots: {heavy}: lockfile fast path fell back ({reason})");
                    }
                    SnapshotOutcome::FellBack(None) => {}
                    SnapshotOutcome::Failed(msg) => {
                        return Err(Error::Store(format!("hydration of {heavy} failed: {msg}")));
                    }
                }
            }
        }

        let stage = Instant::now();
        let ingested = ingest_dir(self.store, root, &src, cfg)?;
        timings.ingest_ms += stage.elapsed().as_millis();

        if cfg.snapshots {
            let stage = Instant::now();
            match snapshots::hydrate(
                self.store,
                &ingested,
                root,
                pattern,
                &src,
                &heavy,
                dest,
                pinned_lockfile_hash.as_ref(),
                cfg,
            ) {
                SnapshotOutcome::Hydrated(h) => {
                    timings.snapshot_ms += stage.elapsed().as_millis();
                    timings.snapshot_engaged = true;
                    timings.snapshot_lookup_ms += h.lookup_ms;
                    timings.snapshot_clonefile_ms += h.clonefile_ms;
                    timings.snapshot_mode = h.mode;
                    timings.v2_cloned += h.cloned_units;
                    timings.v2_linked += h.linked_files;
                    if let Some(b) = h.build {
                        timings.snapshot_built = true;
                        timings.build_verify_ms += b.verify_ms;
                        timings.build_link_train_ms += b.link_train_ms;
                        timings.build_publish_ms += b.publish_ms;
                    }
                    let refs = Instant::now();
                    let git_dir = claim_snapshot_references(self.store, dest, &ingested, h.hash)?;
                    timings.references_ms += refs.elapsed().as_millis();
                    if !cfg.json {
                        println!(
                            "hydrated {heavy} from {} via snapshot {} (one clone, {} file{})",
                            src.display(),
                            &h.hash.to_string()[..12],
                            h.files,
                            if h.files == 1 { "" } else { "s" },
                        );
                    }
                    return Ok(DirOutcome {
                        git_dir,
                        snapshot_hash: Some(h.hash),
                        ingested,
                        ladder: None,
                    });
                }
                SnapshotOutcome::FellBack(Some(reason)) => {
                    eprintln!(
                        "wt-snapshots: {heavy}: falling back to per-file placement ({reason})"
                    );
                }
                SnapshotOutcome::FellBack(None) => {}
                SnapshotOutcome::Failed(msg) => {
                    return Err(Error::Store(format!("hydration of {heavy} failed: {msg}")));
                }
            }
        }

        let stage = Instant::now();
        let git_dir = claim_references(self.store, dest, &ingested)?;
        timings.references_ms += stage.elapsed().as_millis();

        let stage = Instant::now();
        let report = materialize(self.store, &ingested, dest, cfg)
            .map_err(|e| Error::Store(format!("hydration of {} failed: {e}", rel.display())))?;
        timings.materialize_ms += stage.elapsed().as_millis();
        timings.verify_ms += report.verify_ms;
        timings.place_ms += report.place_ms;
        if !cfg.json {
            println!(
                "hydrated {} from {} via store ({} file{})",
                rel.display(),
                src.display(),
                report.files,
                if report.files == 1 { "" } else { "s" }
            );
        }
        Ok(DirOutcome {
            git_dir,
            snapshot_hash: None,
            ingested,
            ladder: Some(report),
        })
    }
}

/// What one heavy directory contributed to the worktree and the mirror.
struct DirOutcome {
    /// The worktree's resolved git dir (captured while claiming references).
    git_dir: PathBuf,
    /// Manifest hash when the snapshot fast path served this directory.
    snapshot_hash: Option<ContentId>,
    ingested: Ingested,
    /// Per-file ladder placement report; `None` means snapshot served the directory.
    ladder: Option<MaterializeReport>,
}

/// Everything one heavy directory contributed to the store.
pub struct Ingested {
    /// Repo-relative directory paths to recreate even when empty.
    pub dirs: Vec<String>,
    /// Repo-relative directory path -> on-disk mode (`& 0o7777`).
    /// Recorded on every ingest; materialize restores these bits
    /// after placement, because `create_dir_all` normalizes new
    /// directories through the process umask just like it once did
    /// for files.
    pub dir_modes: BTreeMap<String, u32>,
    /// Repo-relative file path -> stored content address.
    pub files: BTreeMap<String, ContentId>,
    /// Repo-relative file path -> file size in bytes.
    pub file_sizes: BTreeMap<String, u64>,
    /// Repo-relative symlink path -> raw target (possibly dangling;
    /// targets are never followed or stored as blobs). Recorded on
    /// every ingest so both the fallback ladder and the snapshot
    /// manifest can recreate the link verbatim.
    pub symlinks: BTreeMap<String, String>,
    /// Repo-relative file path -> on-disk mode (`& 0o7777`). Recorded
    /// on every ingest; the fallback ladder restores it after
    /// placement, the snapshot manifest consumes it too.
    pub modes: BTreeMap<String, u32>,
}

/// Walk `src`, storing every regular file's bytes. Symlinks are never
/// followed out of `src`; they are recorded with their raw targets
/// (dangling ones included) so materialize can recreate them
/// verbatim, and non-regular files are skipped — except under
/// `WT_SNAPSHOTS=1`, where anything a manifest cannot represent fails
/// loudly rather than silently vanishing from a snapshot.
///
/// Ticket 02: a validation cache beside the store remembers each
/// path's size, mtime, and content id from the previous ingest. A
/// file whose size AND mtime both still match is not read or hashed —
/// its recorded blob is reused (checked against the store first, in
/// case a sweep reclaimed it). Every other file goes through the
/// normal read-and-hash path. The cache can only make runs cheaper,
/// never wronger: materialize proves every blob before placing it
/// (`ensure_verified`, ticket 05), so lying cached metadata fails
/// loudly instead of landing bad bytes in a fresh tree.
pub fn ingest_dir(
    store: &mut DiskStore,
    src_root: &Path,
    src: &Path,
    cfg: &RunConfig,
) -> Result<Ingested> {
    let snapshots = cfg.snapshots;
    let mut ingested = Ingested {
        dirs: Vec::new(),
        dir_modes: BTreeMap::new(),
        files: BTreeMap::new(),
        file_sizes: BTreeMap::new(),
        symlinks: BTreeMap::new(),
        modes: BTreeMap::new(),
    };
    let mut cache = ValidationCache::open(store.root());

    // macOS fast path (Step 0 follow-up): one getattrlistbulk per
    // directory replaces readdir-plus-per-file-stat — at 40k files
    // that is ~40k fewer syscalls. Only engaged with snapshots on,
    // where every stat'd attribute is actually consumed; any failure
    // silently falls back to the portable walk below.
    #[cfg(target_os = "macos")]
    let walked = if snapshots {
        bulkwalk::walk(src).ok()
    } else {
        None
    };
    // Never constructed off macOS; the type exists only so the match
    // below compiles unchanged.
    #[cfg(not(target_os = "macos"))]
    let walked: Option<std::convert::Infallible> = None;

    match walked {
        #[cfg(target_os = "macos")]
        Some(entries) => {
            // Bulk entries are relative to `src`; every consumer (and
            // the legacy walk) speaks repo-relative paths, so prefix
            // with the heavy directory's own repo-relative name.
            let base = rel_text(src_root, src)?;
            // The heavy root itself is a directory the manifest must
            // carry (its clone replaces it wholesale on the snapshot
            // path, but the per-file ladder recreates it).
            let root_meta = fs::symlink_metadata(src).map_err(|e| Error::io("stat", src, e))?;
            ingested
                .dir_modes
                .insert(base.clone(), root_meta.mode() & 0o7777);
            ingested.dirs.push(base.clone());
            for entry in entries {
                let rel = if base.is_empty() {
                    entry.rel_path.clone()
                } else {
                    format!("{base}/{}", entry.rel_path)
                };
                if crate::toolchain::is_volatile_cache(&rel) {
                    continue;
                }
                let path = src.join(&entry.rel_path);
                if entry.is_symlink {
                    let target =
                        fs::read_link(&path).map_err(|e| Error::io("read symlink", &path, e))?;
                    ingested
                        .symlinks
                        .insert(rel, target.to_string_lossy().into_owned());
                    continue;
                }
                if entry.is_dir {
                    ingested.dir_modes.insert(rel.clone(), entry.mode & 0o7777);
                    ingested.dirs.push(rel);
                    continue;
                }
                if !entry.is_file {
                    if snapshots {
                        return Err(Error::Store(format!(
                            "{} is not a regular file (fifos/sockets/devices are unsupported)",
                            path.display()
                        )));
                    }
                    continue;
                }
                if snapshots {
                    ingested.modes.insert(rel.clone(), entry.mode & 0o7777);
                }
                let mtime = std::time::UNIX_EPOCH
                    + std::time::Duration::new(entry.mtime_secs, entry.mtime_nanos);
                let id = match cache.lookup(&rel, entry.size, mtime) {
                    Some(id) if store.contains(&id) => id,
                    _ => {
                        let bytes = fs::read(&path).map_err(|e| Error::io("read", &path, e))?;
                        let id = store.put(&bytes)?;
                        cache.record(
                            rel.clone(),
                            CacheEntry {
                                size: entry.size,
                                mtime,
                                id,
                            },
                        );
                        id
                    }
                };
                ingested.file_sizes.insert(rel.clone(), entry.size);
                ingested.files.insert(rel, id);
            }
        }
        None => ingest_dir_walk(store, &mut cache, &mut ingested, src_root, src, snapshots)?,
    }

    ingested.dirs.sort();
    ingested.dirs.dedup();
    cache
        .save()
        .map_err(|e| Error::io_unanchored("update ingest cache", store.root(), e))?;
    Ok(ingested)
}

/// The portable read_dir+metadata walk: one `fs::metadata` per regular
/// file on top of each directory's readdir. Also the fallback for the
/// macOS bulk walker.
fn ingest_dir_walk(
    store: &mut DiskStore,
    cache: &mut ValidationCache,
    ingested: &mut Ingested,
    src_root: &Path,
    src: &Path,
    snapshots: bool,
) -> Result<()> {
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rel = rel_text(src_root, &dir)?;
        if crate::toolchain::is_volatile_cache(&rel) {
            continue;
        }
        let entries = fs::read_dir(&dir).map_err(|e| Error::io("read", &dir, e))?;
        // One extra stat per DIRECTORY (not per file): its permission
        // bits must survive hydration, since create_dir_all cannot
        // carry them through the umask.
        let dir_meta = fs::symlink_metadata(&dir).map_err(|e| Error::io("stat", &dir, e))?;
        ingested
            .dir_modes
            .insert(rel.clone(), dir_meta.mode() & 0o7777);
        ingested.dirs.push(rel);
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = rel_text(src_root, &path)?;
            if crate::toolchain::is_volatile_cache(&rel) {
                continue;
            }
            let file_type = entry.file_type().map_err(|e| Error::io("stat", &path, e))?;
            if file_type.is_symlink() {
                // The raw target is what gets recorded, dangling or
                // not: materialize recreates it verbatim via
                // symlink(2), which never resolves the target. (With
                // snapshots on the manifest consumes the same record.)
                let target =
                    fs::read_link(&path).map_err(|e| Error::io("read symlink", &path, e))?;
                ingested.symlinks.insert(
                    rel_text(src_root, &path)?,
                    target.to_string_lossy().into_owned(),
                );
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                // FIFOs, sockets, devices have no place in a snapshot:
                // fail loudly before placement rather than silently
                // dropping content the manifest cannot represent.
                if snapshots {
                    return Err(Error::Store(format!(
                        "{} is not a regular file (fifos/sockets/devices are unsupported)",
                        path.display()
                    )));
                }
                continue;
            }
            let rel = rel_text(src_root, &path)?;
            let meta = fs::metadata(&path).map_err(|e| Error::io("stat", &path, e))?;
            ingested.modes.insert(rel.clone(), meta.mode() & 0o7777);
            let size = meta.len();
            let mtime = meta.modified().map_err(|e| Error::io("stat", &rel, e))?;
            let id = match cache.lookup(&rel, size, mtime) {
                // Cache hit: same size and same mtime as last time.
                // Trust it only while the blob is actually still here.
                Some(id) if store.contains(&id) => id,
                _ => {
                    // Miss (or a swept blob): pay for read and hash.
                    let bytes = fs::read(&path).map_err(|e| Error::io("read", &path, e))?;
                    let id = store.put(&bytes)?;
                    // An mtime before the epoch cannot round-trip
                    // through the cache format; skip caching rather
                    // than fail, so such a file just stays cold.
                    if mtime >= std::time::UNIX_EPOCH {
                        cache.record(rel.clone(), CacheEntry { size, mtime, id });
                    }
                    id
                }
            };
            ingested.file_sizes.insert(rel.clone(), size);
            ingested.files.insert(rel, id);
        }
    }
    Ok(())
}

/// Repo-relative text of `path` under `root`, or a loud error when a
/// pattern somehow matched outside the ingestion root.
fn rel_text(root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(root)
        .map_err(|_| {
            Error::Store(format!(
                "pattern matched path outside repository root: {}",
                path.display()
            ))
        })
        .map(|p| p.to_string_lossy().into_owned())
}

/// Outcome of materializing one heavy directory via the per-file ladder.
pub struct MaterializeReport {
    /// Total files placed.
    pub files: usize,
    /// Files written as plain byte copies because the selected
    /// strategy was disabled, refused by the filesystem, or could not
    /// carry the recorded mode.
    pub copied: usize,
    /// Bytes placed via shared copy-on-write / hardlink inodes.
    pub bytes_shared: u64,
    /// Bytes written as plain byte copies.
    pub bytes_copied: u64,
    /// Strategy attempted for this directory.
    pub strategy: &'static str,
    /// Milliseconds spent proving blobs before placement.
    pub verify_ms: u128,
    /// Milliseconds spent placing files.
    pub place_ms: u128,
}

/// Recreate the ingested tree under `dest_root` from store content.
///
/// Per file: verify first, and only then place anything. Verification is
/// `Store::get`'s read-and-hash when `RunConfig::verify` is set, and
/// `DiskStore::ensure_verified` otherwise.
///
/// Placement uses [`Materializer`], attempting the selected strategy (CoW clone /
/// reflink / copy_file_range) and falling back to sequential buffered copy.
/// Placement is dispatched in parallel across worker threads.
pub fn materialize(
    store: &DiskStore,
    ingested: &Ingested,
    dest_root: &Path,
    cfg: &RunConfig,
) -> Result<MaterializeReport> {
    let store_caps = store.fs_capabilities();
    let dest_caps = wt_store::probe_fs(dest_root).ok();
    let is_cross_device = dest_caps
        .map(|d| d.device_id != store_caps.device_id)
        .unwrap_or(false);

    let materializer = Materializer::new(
        match cfg.strategy_policy {
            StrategyPolicy::Default => wt_copy::StrategyPolicy::Default,
            StrategyPolicy::Hardlink => wt_copy::StrategyPolicy::Hardlink,
            StrategyPolicy::ForceByteCopy => wt_copy::StrategyPolicy::ForceByteCopy,
        },
        dest_root,
        is_cross_device,
        store_caps.reflink_capable,
        store_caps.is_ext4(),
    );
    let strategy_name = materializer.strategy();
    let paranoid = cfg.verify;

    for rel in &ingested.dirs {
        fs::create_dir_all(dest_root.join(rel))
            .map_err(|e| Error::io("prepare", dest_root.join(rel), e))?;
    }

    let files: Vec<(&String, &ContentId)> = ingested.files.iter().collect();
    let num_cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let num_workers = num_cpus.clamp(4, 8).min(files.len()).max(1);

    let next_file_idx = std::sync::atomic::AtomicUsize::new(0);
    let total_copied = std::sync::atomic::AtomicUsize::new(0);
    let total_bytes_shared = std::sync::atomic::AtomicU64::new(0);
    let total_bytes_copied = std::sync::atomic::AtomicU64::new(0);
    let total_verify_ms = std::sync::atomic::AtomicU64::new(0);
    let total_place_ms = std::sync::atomic::AtomicU64::new(0);
    let err_slot: std::sync::Mutex<Option<Error>> = std::sync::Mutex::new(None);

    std::thread::scope(|s| {
        for _ in 0..num_workers {
            s.spawn(|| {
                let mut worker_verify_ms = 0u128;
                let mut worker_place_ms = 0u128;
                let mut worker_copied = 0usize;
                let mut worker_bytes_shared = 0u64;
                let mut worker_bytes_copied = 0u64;

                loop {
                    if err_slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                        break;
                    }

                    let idx = next_file_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if idx >= files.len() {
                        break;
                    }

                    let (rel, id) = files[idx];
                    let size = ingested.file_sizes.get(rel).copied().unwrap_or(0);

                    let v_start = Instant::now();
                    let v_res = if paranoid {
                        store.get(id).map(|_| ())
                    } else {
                        store.ensure_verified(id)
                    };
                    if let Err(e) = v_res {
                        let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                        if slot.is_none() {
                            *slot = Some(Error::Store(format!("materialize {rel}: {e}")));
                        }
                        break;
                    }
                    worker_verify_ms += v_start.elapsed().as_millis();

                    let src = store.blob_path(id);
                    let dest = dest_root.join(rel);
                    let mode = ingested.modes.get(rel).copied();

                    let p_start = Instant::now();
                    let outcome = match materializer.materialize_file(&src, &dest, mode) {
                        Ok(outcome) => outcome,
                        Err(e) => {
                            let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                            if slot.is_none() {
                                *slot = Some(Error::Store(format!("materialize {rel}: {e}")));
                            }
                            break;
                        }
                    };
                    worker_place_ms += p_start.elapsed().as_millis();

                    if outcome.is_shared_cow {
                        worker_bytes_shared += size;
                    } else {
                        worker_copied += 1;
                        worker_bytes_copied += size;
                    }
                }

                total_copied.fetch_add(worker_copied, std::sync::atomic::Ordering::Relaxed);
                total_bytes_shared
                    .fetch_add(worker_bytes_shared, std::sync::atomic::Ordering::Relaxed);
                total_bytes_copied
                    .fetch_add(worker_bytes_copied, std::sync::atomic::Ordering::Relaxed);
                total_verify_ms.fetch_add(
                    worker_verify_ms as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                total_place_ms
                    .fetch_add(worker_place_ms as u64, std::sync::atomic::Ordering::Relaxed);
            });
        }
    });

    if let Some(err) = err_slot.into_inner().unwrap_or_default() {
        return Err(err);
    }

    for (rel, target) in &ingested.symlinks {
        let dest = dest_root.join(rel);
        if let Some(parent) = dest.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| Error::io("prepare", parent, e))?;
            }
        }
        std::os::unix::fs::symlink(target, &dest)
            .map_err(|e| Error::io("place symlink", &dest, e))?;
    }

    for rel in ingested.dirs.iter().rev() {
        let Some(&mode) = ingested.dir_modes.get(rel) else {
            continue;
        };
        let path = dest_root.join(rel);
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .map_err(|e| Error::io("restore dir mode", &path, e))?;
    }

    Ok(MaterializeReport {
        files: ingested.files.len(),
        copied: total_copied.into_inner(),
        bytes_shared: total_bytes_shared.into_inner(),
        bytes_copied: total_bytes_copied.into_inner(),
        strategy: strategy_name,
        verify_ms: total_verify_ms.into_inner() as u128,
        place_ms: total_place_ms.into_inner() as u128,
    })
}

/// Resolve the (absolute) git dir of a freshly created worktree.
fn worktree_git_dir(worktree: &Path) -> Result<PathBuf> {
    gitops::git_dir(worktree)
}

/// Give this worktree one reference on every distinct blob it uses,
/// then record the mapping where ticket 06 can find it.
///
/// Ticket 07: in `mark-sweep-no-refs` mode the refcount writes are
/// skipped entirely — mirrors are the only bookkeeping. The sidecar
/// is written in every mode; it stays the diagnostic/recovery record
/// and the liveness evidence sweep validates roots against.
pub fn claim_references(
    store: &mut DiskStore,
    worktree: &Path,
    ingested: &Ingested,
) -> Result<PathBuf> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    if store.gc_mode() != GcMode::MarkSweepNoRefs {
        for id in &distinct {
            store.add_ref(id)?;
        }
    }

    let git_dir = worktree_git_dir(worktree)?;
    let sidecar_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))
        .map_err(|e| {
            Error::io_unanchored("open hydration ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    // 40k one-line writes through an unbuffered handle cost 40k
    // write(2) syscalls; a BufWriter turns that into a handful of
    // large ones.
    let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
    for (rel, id) in &ingested.files {
        writeln!(sidecar, "{rel}\t{id}").map_err(|e| {
            Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    }
    sidecar
        .flush()
        .map_err(|e| Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e))?;
    Ok(git_dir)
}

/// Publish the authoritative store-local mirror for one successful
/// create: one atomic write naming the worktree's canonical identity
/// and every blob or snapshot it hydrates from (ticket 07). This —
/// not the per-blob refcounts — is what mark-and-sweep marks through.
/// A snapshot hydration contributes ONLY its `snapshot` record; the
/// manifest marks every child blob.
pub fn publish_mirror(
    store: &mut DiskStore,
    worktree: &Path,
    git_dir: &Path,
    ingested: &Ingested,
    snapshots: &[ContentId],
    base_branch: Option<&str>,
    base_commit: Option<&str>,
) -> Result<()> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    store
        .publish_worktree_mirror(
            worktree,
            git_dir,
            distinct,
            snapshots.iter(),
            base_branch,
            base_commit,
        )
        .map(|_| ())
        .map_err(|e| Error::Store(format!("cannot publish worktree mirror: {e}")))
}

/// Ticket 08: give a snapshot-hydrated worktree its bookkeeping.
///
/// Legacy refs are still claimed on every distinct child blob (dual-
/// write safety: an old binary reading the store must not collect
/// blobs it cannot see through snapshot manifests), but the sidecar
/// carries TYPED rows so removal knows which ids are blobs and which
/// name snapshots:
///
/// ```text
/// <rel><TAB>blob<TAB><64-hex-blob-id>
/// -<TAB>snapshot<TAB><64-hex-manifest-hash>
/// ```
///
/// (Two-field rows remain the legacy per-file format from ticket 05.)
/// The store-local mirror itself gets only the `snapshot` record.
pub fn claim_snapshot_references(
    store: &mut DiskStore,
    worktree: &Path,
    ingested: &Ingested,
    snapshot: ContentId,
) -> Result<PathBuf> {
    use wt_store::Store as _;

    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    if store.gc_mode() != GcMode::MarkSweepNoRefs {
        for id in &distinct {
            store.add_ref(id)?;
        }
    }

    let git_dir = worktree_git_dir(worktree)?;
    let sidecar_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))
        .map_err(|e| {
            Error::io_unanchored("open hydration ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    // Same buffering rationale as claim_references.
    let mut sidecar = io::BufWriter::with_capacity(128 * 1024, sidecar_file);
    for (rel, id) in &ingested.files {
        writeln!(sidecar, "{rel}\tblob\t{id}").map_err(|e| {
            Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e)
        })?;
    }
    writeln!(sidecar, "-\tsnapshot\t{snapshot}")
        .map_err(|e| Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e))?;
    sidecar
        .flush()
        .map_err(|e| Error::io_unanchored("write ledger", git_dir.join("wt-hydrated.tsv"), e))?;
    Ok(git_dir)
}
