//! Whole-directory snapshot hydration (fast-hydration ticket 08,
//! Phase 2 of AGENT_HANDOFF_PLAN_REVISED.md).
//!
//! The fast path for one heavy directory, behind `WT_SNAPSHOTS=1`:
//!
//! ```text
//! ingest -> canonical manifest -> manifest hash
//!   valid published snapshot with that hash?  --yes--> clonefile it out
//!      | no                                        (ONE syscall, private inodes)
//!      no: verify every blob per policy, hardlink them into
//!          snapshots/tmp/<uuid>, write manifest + .complete,
//!          atomically rename into place, then clonefile it out
//! ```
//!
//! A snapshot hit performs zero blob reads; integrity was proven at
//! publish time. `WT_VERIFY=1` bypasses hits entirely and rebuilds
//! from freshly hashed blobs. Anything the filesystem refuses — cross-
//! device destinations, non-APFS volumes, a partial clone — falls back
//! to the existing per-file ladder with `file` mirror records instead
//! of a `snapshot` record, exactly as if the gate were off.

#[cfg(target_os = "macos")]
use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::Instant;

// Used by every platform: hydrate() itself plus the Outcome payload.
use wt_store::{ContentId, DiskStore, SnapshotBuildTiming};

#[cfg(target_os = "macos")]
use wt_copy::{ClonefileBackend, CopyBackend};
#[cfg(target_os = "macos")]
use wt_store::{
    BuildError, Manifest, PublishOutcome, SnapshotDiff, SnapshotEntry, select_old_snapshot,
};

use crate::config::RunConfig;
use crate::hydrate::Ingested;

/// One heavy directory hydrated through the fast path.
pub struct SnapshotHydration {
    /// Manifest hash of the published snapshot that was cloned.
    pub hash: ContentId,
    /// Regular files the snapshot carried (for run reporting).
    pub files: usize,
    /// How this directory was served: `"hit"` (cloned an existing
    /// published snapshot), `"build"` (built + published from blobs),
    /// or `"v2"` (incremental rebuild off the previous snapshot's
    /// tree).
    pub mode: &'static str,
    /// v2 only: 1 when the rebuild cloned the previous snapshot's
    /// whole tree with one recursive clonefile, else 0. Zero for
    /// hit/build.
    pub cloned_units: usize,
    /// v2 only: regular files freshly hardlinked from blobs while
    /// applying the delta. Zero for hit/build.
    pub linked_files: usize,
    /// Step 0 instrumentation: milliseconds spent looking up a
    /// published snapshot (all attempts, hit or miss).
    pub lookup_ms: u128,
    /// Step 0 instrumentation: milliseconds spent clonefile-ing the
    /// tree out (hit clone and/or post-build clone).
    pub clonefile_ms: u128,
    /// Step 0 instrumentation: internal build-phase timings, present
    /// only when this run built and published the snapshot.
    pub build: Option<SnapshotBuildTiming>,
}

/// Result of attempting the snapshot path for one heavy directory.
pub enum Outcome {
    /// Hydrated via one recursive clone of a published snapshot.
    /// The caller records the hash in sidecar and mirror.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Hydrated(SnapshotHydration),
    /// The snapshot path could not serve this directory; run the
    /// existing per-file ladder instead. Carries a diagnostic when
    /// one exists (printed once, never fatal).
    FellBack(Option<String>),
    /// Something went wrong that must NOT be silently papered over:
    /// fail the create loudly.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Failed(String),
}

/// Attempt an O(1) fast-path snapshot hydration without walking or ingesting
/// the heavy directory when a pinned lockfile SHA-256 matches a published
/// snapshot manifest header and the heavy root mtime is unchanged.
pub fn try_lockfile_hit(
    store: &mut DiskStore,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: &ContentId,
    cfg: &RunConfig,
) -> Outcome {
    #[cfg(target_os = "macos")]
    {
        try_lockfile_hit_impl(
            store, repo_root, pattern, src_root, heavy_rel, dest_root, lockfile_hash, cfg,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            store, repo_root, pattern, src_root, heavy_rel, dest_root, lockfile_hash, cfg,
        );
        Outcome::FellBack(None)
    }
}

#[cfg(target_os = "macos")]
fn try_lockfile_hit_impl(
    store: &mut DiskStore,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: &ContentId,
    cfg: &RunConfig,
) -> Outcome {
    if cfg.verify {
        return Outcome::FellBack(None);
    }
    let backend = ClonefileBackend;
    if !backend.supports(dest_root) {
        return Outcome::FellBack(None);
    }
    let repo_key = repo_root.to_string_lossy().into_owned();
    let idx = wt_store::SelectionIndex::load(store.root());
    let Some(rec) = idx.records.iter().find(|r| r.matches(&repo_key, pattern, heavy_rel)) else {
        return Outcome::FellBack(None);
    };

    let heavy_src = src_root.join(heavy_rel);
    let Ok(src_meta) = fs::symlink_metadata(&heavy_src) else {
        return Outcome::FellBack(None);
    };
    if !src_meta.is_dir() {
        return Outcome::FellBack(None);
    }
    let src_mtime_secs = src_meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Root directory modification times must be unchanged since snapshot publish
    if rec.mtime_secs > 0 && src_mtime_secs > rec.mtime_secs + 1 {
        return Outcome::FellBack(None);
    }

    let mut lookup_ms = 0u128;
    let mut clonefile_ms = 0u128;

    for hash in &rec.ring {
        let stage = Instant::now();
        let candidate = store.find_snapshot(hash);
        lookup_ms += stage.elapsed().as_millis();
        if let Some(manifest) = candidate {
            if manifest.lockfile_hash == Some(*lockfile_hash) {
                let dest_heavy = dest_root.join(heavy_rel);
                let tree = wt_store::snapshot_tree_path(store.root(), hash);
                let stage = Instant::now();
                let cloned = finish_clone(&backend, &tree, &dest_heavy);
                clonefile_ms += stage.elapsed().as_millis();
                match cloned {
                    Ok(()) => {
                        let _ = wt_store::record_snapshot_hit(
                            store.root(),
                            &repo_key,
                            pattern,
                            heavy_rel,
                            hash,
                        );
                        let file_count = manifest
                            .entries
                            .iter()
                            .filter(|e| e.kind == wt_store::EntryKind::File)
                            .count();
                        return Outcome::Hydrated(SnapshotHydration {
                            hash: *hash,
                            files: file_count,
                            mode: "hit",
                            cloned_units: 0,
                            linked_files: 0,
                            lookup_ms,
                            clonefile_ms,
                            build: None,
                        });
                    }
                    Err(CloneFailure::SnapshotVanished) => continue,
                    Err(CloneFailure::Refused(diag)) => return Outcome::FellBack(diag),
                    Err(CloneFailure::Fatal(msg)) => return Outcome::Failed(msg),
                }
            }
        }
    }

    Outcome::FellBack(None)
}

/// Build and hydrate one heavy directory through snapshots, or fall
/// back. `src_root` is where ingested content came from (for blob
/// healing), `dest_root` the new worktree root, `heavy_rel` the
/// repo-relative heavy directory name ("heavy" in `heavy/pkg/x`).
/// `repo_root` and `pattern` key the v2 selection index (the first
/// `.wtinclude` pattern that matched this directory is fine; the key
/// only needs to be stable across runs). `cfg.verify` bypasses hits;
/// v2 incremental rebuilds additionally require `cfg.snapshots` and
/// `cfg.v2`.
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
) -> Outcome {
    #[cfg(target_os = "macos")]
    {
        hydrate_impl(
            store, ingested, repo_root, pattern, src_root, heavy_rel, dest_root,
            lockfile_hash, cfg,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            store, ingested, repo_root, pattern, src_root, heavy_rel, dest_root,
            lockfile_hash, cfg,
        );
        // Linux v1: no recursive-clone primitive, so whole-directory
        // snapshots stay a macOS feature. The gate is a no-op.
        Outcome::FellBack(None)
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn hydrate_impl(
    store: &mut DiskStore,
    ingested: &Ingested,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: Option<&ContentId>,
    cfg: &RunConfig,
) -> Outcome {
    let paranoid = cfg.verify;
    let backend = ClonefileBackend;
    // Gate is a no-op on filesystems without recursive clone support.
    if !backend.supports(dest_root) {
        return Outcome::FellBack(None);
    }
    // v2 needs the same APFS substrate PLUS its own env gate.
    let v2 = cfg.snapshots && cfg.v2;
    let repo_key = repo_root.to_string_lossy().into_owned();

    let entries = match manifest_entries(ingested, heavy_rel) {
        Ok(entries) => entries,
        Err(msg) => return Outcome::Failed(msg),
    };
    let manifest = match Manifest::new_with_lockfile(entries, lockfile_hash.copied()) {
        Ok(m) => m,
        Err(msg) => return Outcome::Failed(format!("cannot build snapshot manifest: {msg}")),
    };

    let dest_heavy = dest_root.join(heavy_rel);

    let mut lookup_ms = 0u128;
    let mut clonefile_ms = 0u128;
    let mut build: Option<SnapshotBuildTiming> = None;

    // Two attempts total: a sweep may evict the published snapshot
    // between lookup and clone (ENOENT); the retry rebuilds it.
    for attempt in 0..2u8 {
        if !paranoid && attempt == 0 {
            let stage = Instant::now();
            let published = store.find_snapshot(&manifest.hash);
            lookup_ms += stage.elapsed().as_millis();
            if published.is_some() {
                // HIT: trust verified-at-publish, zero blob reads.
                if v2 {
                    // Index bookkeeping must never fail a create.
                    let _ = wt_store::record_snapshot_hit(
                        store.root(),
                        &repo_key,
                        pattern,
                        heavy_rel,
                        &manifest.hash,
                    );
                } else {
                    // No selection index without v2, but a hit must
                    // still refresh LRU recency or retention-cap
                    // eviction can collect a hot snapshot.
                    wt_store::record_snapshot_lru_touch(store.root(), &manifest.hash);
                }
                let tree = wt_store::snapshot_tree_path(store.root(), &manifest.hash);
                let stage = Instant::now();
                let cloned = finish_clone(&backend, &tree, &dest_heavy);
                clonefile_ms += stage.elapsed().as_millis();
                match cloned {
                    Ok(()) => {
                        return Outcome::Hydrated(SnapshotHydration {
                            hash: manifest.hash,
                            files: ingested.files.len(),
                            mode: "hit",
                            cloned_units: 0,
                            linked_files: 0,
                            lookup_ms,
                            clonefile_ms,
                            build,
                        });
                    }
                    // Snapshot evicted mid-flight: rebuild and try again.
                    Err(CloneFailure::SnapshotVanished) => continue,
                    Err(CloneFailure::Refused(diag)) => return Outcome::FellBack(diag),
                    Err(CloneFailure::Fatal(msg)) => return Outcome::Failed(msg),
                }
            }
        }

        // MISS (or paranoid rebuild). With the v2 gate on, first try
        // an incremental rebuild against a recent old snapshot; ANY
        // failure in that path degrades to the plain full build.
        let mut incremental: Option<(usize, usize)> = None;
        if v2 {
            incremental = try_incremental(
                store, &manifest, &repo_key, pattern, heavy_rel, paranoid, &mut build,
            );
        }

        if incremental.is_none() {
            // FULL BUILD + publish, healing at most one swept-away
            // blob per plan's link(2)-ENOENT backstop.
            match ensure_published(store, ingested, src_root, &manifest, paranoid, &mut build) {
                Ok(PublishOutcome::Published | PublishOutcome::WinnerValid) => {}
                Ok(PublishOutcome::WinnerInvalid) => {
                    // Debris sits on our final name; never overwrite what
                    // we cannot prove ours. Per-file ladder keeps working.
                    return Outcome::FellBack(Some(format!(
                        "snapshot {} exists but is invalid debris; using per-file placement",
                        manifest.hash
                    )));
                }
                Err(msg) => return Outcome::Failed(msg),
            }
        }

        if v2 {
            // Every successful publish — full or incremental — moves
            // the hash to the front of the selection ring.
            let _ = wt_store::record_snapshot_publish(
                store.root(),
                &repo_key,
                pattern,
                heavy_rel,
                &manifest.hash,
            );
        } else {
            // Same recency rule as the hit path above.
            wt_store::record_snapshot_lru_touch(store.root(), &manifest.hash);
        }

        let stage = Instant::now();
        let cloned = finish_clone(
            &backend,
            &wt_store::snapshot_tree_path(store.root(), &manifest.hash),
            &dest_heavy,
        );
        clonefile_ms += stage.elapsed().as_millis();
        match cloned {
            Ok(()) => {
                let (cloned_units, linked_files) = incremental.unwrap_or((0, 0));
                return Outcome::Hydrated(SnapshotHydration {
                    hash: manifest.hash,
                    files: ingested.files.len(),
                    mode: if incremental.is_some() { "v2" } else { "build" },
                    cloned_units,
                    linked_files,
                    lookup_ms,
                    clonefile_ms,
                    build,
                });
            }
            // Snapshot evicted mid-flight: rebuild and try again.
            Err(CloneFailure::SnapshotVanished) => continue,
            Err(CloneFailure::Refused(diag)) => return Outcome::FellBack(diag),
            Err(CloneFailure::Fatal(msg)) => return Outcome::Failed(msg),
        }
    }
    Outcome::Failed(format!(
        "snapshot {} kept vanishing between publish and clone",
        manifest.hash
    ))
}

/// The v2 attempt: pick an old snapshot from the selection index,
/// diff it against the new manifest, and rebuild incrementally when
/// that looks cheaper than a full build.
///
/// Heuristic: go incremental only when fewer than HALF of the new
/// manifest's entries changed (`changed_count * 2 < total`). The
/// whole-tree clone costs roughly the same regardless of the diff, but
/// applying a huge delta (an unlink+relink per changed entry) starts
/// to approach full-build cost while adding clone overhead on top.
/// Deliberately crude — entry counts stand in for bytes.
///
/// Returns `Some((cloned_units, linked_files))` when an incremental
/// rebuild published successfully (or consumed a valid winner):
/// `cloned_units` is 0 or 1 (the single whole-tree clone),
/// `linked_files` counts regular files freshly hardlinked from blobs
/// while applying the delta. EVERYTHING else — no old snapshot,
/// heuristic says no, corrupt old snapshot, refused whole-tree clone,
/// missing blob, paranoid rot detected, rename lost to debris —
/// returns `None`, handing control back to the battle-tested full
/// build whose outcome semantics then apply unchanged.
#[cfg(target_os = "macos")]
fn try_incremental(
    store: &mut DiskStore,
    manifest: &Manifest,
    repo_key: &str,
    pattern: &str,
    heavy_rel: &str,
    paranoid: bool,
    build: &mut Option<SnapshotBuildTiming>,
) -> Option<(usize, usize)> {
    let (old_hash, old_manifest) = select_old_snapshot(store.root(), repo_key, pattern, heavy_rel)?;
    let diff = SnapshotDiff::compute(&old_manifest.entries, &manifest.entries);
    let total = manifest.entries.len();
    if total == 0 || diff.changed_count() * 2 >= total {
        return None;
    }

    match store.publish_snapshot_incremental_with_lockfile_and_timing(
        manifest.entries.clone(),
        manifest.lockfile_hash,
        &old_hash,
        paranoid,
    ) {
        Ok(receipt) => {
            let timing = receipt.timing;
            *build = Some(timing);
            Some((timing.clone_units, timing.linked_files))
        }
        _ => None,
    }
}

#[cfg(target_os = "macos")]
enum CloneFailure {
    /// The published directory disappeared mid-flight (eviction race).
    SnapshotVanished,
    /// Filesystem refused; fall back to the per-file ladder.
    Refused(Option<String>),
    /// Real failure: loud error after cleaning our partial tree.
    Fatal(String),
}

/// Clone the published tree to `dest_heavy`, honoring the plan's
/// destination rules: absent -> clone; empty-and-command-owned ->
/// remove and clone; non-empty -> merge via the per-file ladder.
/// Any partial clone result is removed — only ever a tree this
/// command owned.
#[cfg(target_os = "macos")]
fn finish_clone(
    backend: &ClonefileBackend,
    tree: &Path,
    dest_heavy: &Path,
) -> Result<(), CloneFailure> {
    // Destination rules. A dangling symlink counts as existing junk we
    // did not create: fall back rather than clobber.
    let mut restore_empty_dir = false;
    match fs::symlink_metadata(dest_heavy) {
        Ok(md) if md.is_dir() => {
            let mut contents = fs::read_dir(dest_heavy).map_err(|e| {
                CloneFailure::Fatal(format!("cannot inspect {}: {e}", dest_heavy.display()))
            })?;
            if contents.next().is_some() {
                // Non-empty: existing per-file merge behavior owns this
                // case; the snapshot path never merges.
                return Err(CloneFailure::Refused(None));
            }
            // Empty directory created before hydration (git checkout
            // of tracked paths): remove so clonefile can land, then
            // recreate if anything below refuses.
            fs::remove_dir(dest_heavy).map_err(|e| {
                CloneFailure::Fatal(format!("cannot clear empty {}: {e}", dest_heavy.display()))
            })?;
            restore_empty_dir = true;
        }
        Ok(_) => {
            return Err(CloneFailure::Refused(Some(format!(
                "{} exists and is not a directory",
                dest_heavy.display()
            ))));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(CloneFailure::Fatal(format!(
                "cannot stat {}: {e}",
                dest_heavy.display()
            )));
        }
    }

    match backend.copy_dir(tree, dest_heavy) {
        Ok(()) => Ok(()),
        Err(wt_copy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            // Source snapshot vanished mid-flight (eviction race).
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::SnapshotVanished)
        }
        Err(wt_copy::Error::DestinationExists) => {
            // Raced with something else creating the destination:
            // its content is not ours to remove; merge instead.
            Err(CloneFailure::Refused(None))
        }
        // Cross-device, unsupported filesystem, permission-shaped
        // refusal: the existing per-file ladder preserves semantics.
        Err(wt_copy::Error::Unsupported | wt_copy::Error::UnsafeBackend) => {
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::Refused(None))
        }
        Err(wt_copy::Error::Io(e)) => {
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::Fatal(format!(
                "clonefile {} -> {}: {e}",
                tree.display(),
                dest_heavy.display()
            )))
        }
    }
}

/// Remove a partial destination tree this command created (the
/// destination was absent, or an empty dir we cleared ourselves).
#[cfg(target_os = "macos")]
fn cleanup_partial(dest_heavy: &Path, restore_empty_dir: bool) -> Result<(), CloneFailure> {
    match fs::symlink_metadata(dest_heavy) {
        Ok(_) => {
            fs::remove_dir_all(dest_heavy).map_err(|e| {
                CloneFailure::Fatal(format!(
                    "cannot clean partial clone at {}: {e}",
                    dest_heavy.display()
                ))
            })?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(CloneFailure::Fatal(format!(
                "cannot stat partial clone at {}: {e}",
                dest_heavy.display()
            )));
        }
    }
    if restore_empty_dir {
        fs::create_dir(dest_heavy).map_err(|e| {
            CloneFailure::Fatal(format!(
                "cannot restore empty {}: {e}",
                dest_heavy.display()
            ))
        })?;
    }
    Ok(())
}

/// Canonical manifest inputs from ingested content, relative to the
/// heavy directory itself (the snapshot tree REPLACES that directory,
/// so its root carries no name).
#[cfg(target_os = "macos")]
fn manifest_entries(ingested: &Ingested, heavy_rel: &str) -> Result<Vec<SnapshotEntry>, String> {
    let prefix = format!("{heavy_rel}/");
    let strip = |path: &str| -> Result<String, String> {
        path.strip_prefix(&prefix)
            .map(str::to_owned)
            .ok_or_else(|| format!("ingested path {path:?} lies outside {heavy_rel:?}"))
    };
    let mut out = Vec::new();
    // Every walked directory appears explicitly; skip the heavy root
    // itself (it IS the manifest root). Empty dirs survive as `dir`
    // entries.
    for dir in &ingested.dirs {
        if dir == heavy_rel {
            continue;
        }
        out.push(SnapshotEntry::dir(strip(dir)?));
    }
    for (path, id) in &ingested.files {
        let rel = strip(path)?;
        let mode = ingested.modes.get(path).copied().unwrap_or(0o644);
        out.push(SnapshotEntry::file(rel, *id, mode));
    }
    for (path, target) in &ingested.symlinks {
        out.push(SnapshotEntry::symlink(strip(path)?, target));
    }
    Ok(out)
}

/// Make sure a valid published snapshot exists for `manifest`,
/// building and publishing it when missing. Verifies every file blob
/// per policy inside [`DiskStore::publish_snapshot`]; heals one
/// sweep-race ENOENT by re-running put() from the source file.
/// On success, fills `build` with the winning attempt's internal
/// phase timings (Step 0 instrumentation).
#[cfg(target_os = "macos")]
fn ensure_published(
    store: &mut DiskStore,
    ingested: &Ingested,
    src_root: &Path,
    manifest: &Manifest,
    paranoid: bool,
    build: &mut Option<SnapshotBuildTiming>,
) -> Result<PublishOutcome, String> {
    let mut healed = false;
    loop {
        match store.publish_snapshot_with_lockfile_and_timing(
            manifest.entries.clone(),
            manifest.lockfile_hash,
            paranoid,
        ) {
            Ok(receipt) => {
                *build = Some(receipt.timing);
                return Ok(receipt.outcome);
            }
            Err(BuildError::MissingBlob(blob)) if !healed => {
                // Sweep raced us: re-run put() for the source content,
                // then retry ONCE. put re-records the fingerprint, so
                // re-verification happens inside the next build.
                healed = true;
                heal_blob(store, ingested, src_root, blob)?;
            }
            Err(BuildError::MissingBlob(blob)) => {
                return Err(format!("blob {blob} vanished twice during snapshot build"));
            }
            Err(BuildError::Fatal(msg)) => return Err(msg),
            Err(BuildError::Io(e)) => return Err(e.to_string()),
        }
    }
}

/// Re-store the source bytes behind `blob`. Ingest recorded which
/// source file produced each id; find it and read it fresh.
#[cfg(target_os = "macos")]
fn heal_blob(
    store: &mut DiskStore,
    ingested: &Ingested,
    src_root: &Path,
    blob: ContentId,
) -> Result<(), String> {
    use wt_store::Store as _;

    let rel = ingested
        .files
        .iter()
        .find(|(_, id)| **id == blob)
        .map(|(rel, _)| rel.clone())
        .ok_or_else(|| format!("blob {blob} vanished but has no known source to heal from"))?;
    let bytes = fs::read(src_root.join(&rel))
        .map_err(|e| format!("cannot re-read {rel} for blob healing: {e}"))?;
    store.put(&bytes).map_err(|e| e.to_string())?;
    Ok(())
}
