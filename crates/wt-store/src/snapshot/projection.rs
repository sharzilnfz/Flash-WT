//! Snapshot projection engine: selection heuristics, v2 delta rebuild
//! calculations, selection index ring updates, LRU touches, self-healing
//! blob retries, lockfile fast-path hits, and clone placement.

use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::Instant;

#[cfg(target_os = "macos")]
use crate::snapdiff::SnapshotDiff;
#[cfg(target_os = "macos")]
use crate::snapindex::select_old_snapshot;
use crate::snapshot::SnapshotBuildTiming;
#[cfg(target_os = "macos")]
use crate::snapshot::{BuildError, Manifest, PublishOutcome, SnapshotEntry, snapshot_tree_path};
use crate::{ContentId, DiskStore};
#[cfg(target_os = "macos")]
use wt_copy::{ClonefileBackend, CopyBackend};

/// One heavy directory hydrated through the fast path.
#[derive(Debug, Clone)]
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
#[derive(Debug)]
pub enum SnapshotOutcome {
    /// Hydrated via one recursive clone of a published snapshot.
    /// The caller records the hash in sidecar and mirror.
    Hydrated(SnapshotHydration),
    /// The snapshot path could not serve this directory; run the
    /// existing per-file ladder instead. Carries a diagnostic when
    /// one exists (printed once, never fatal).
    FellBack(Option<String>),
    /// Something went wrong that must NOT be silently papered over:
    /// fail the create loudly.
    Failed(String),
}

/// Projection engine for whole-directory snapshot hydration.
pub struct SnapshotProjectionEngine;

impl SnapshotProjectionEngine {
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
        verify: bool,
    ) -> SnapshotOutcome {
        #[cfg(target_os = "macos")]
        {
            try_lockfile_hit_impl(
                store,
                repo_root,
                pattern,
                src_root,
                heavy_rel,
                dest_root,
                lockfile_hash,
                verify,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (
                store,
                repo_root,
                pattern,
                src_root,
                heavy_rel,
                dest_root,
                lockfile_hash,
                verify,
            );
            SnapshotOutcome::FellBack(None)
        }
    }

    /// Build and hydrate one heavy directory through snapshots, or fall
    /// back. `src_root` is where ingested content came from (for blob
    /// healing), `dest_root` the new worktree root, `heavy_rel` the
    /// repo-relative heavy directory name ("heavy" in `heavy/pkg/x`).
    /// `repo_root` and `pattern` key the v2 selection index. `verify`
    /// bypasses hits; v2 incremental rebuilds require `snapshots_enabled`
    /// and `v2_enabled`.
    #[allow(clippy::too_many_arguments)]
    pub fn hydrate(
        store: &mut DiskStore,
        dirs: &[String],
        _dir_modes: &BTreeMap<String, u32>,
        files: &BTreeMap<String, ContentId>,
        file_sizes: &BTreeMap<String, u64>,
        symlinks: &BTreeMap<String, String>,
        modes: &BTreeMap<String, u32>,
        repo_root: &Path,
        pattern: &str,
        src_root: &Path,
        heavy_rel: &str,
        dest_root: &Path,
        lockfile_hash: Option<&ContentId>,
        verify: bool,
        snapshots_enabled: bool,
        v2_enabled: bool,
    ) -> SnapshotOutcome {
        #[cfg(target_os = "macos")]
        {
            hydrate_impl(
                store,
                dirs,
                files,
                file_sizes,
                symlinks,
                modes,
                repo_root,
                pattern,
                src_root,
                heavy_rel,
                dest_root,
                lockfile_hash,
                verify,
                snapshots_enabled,
                v2_enabled,
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (
                store,
                dirs,
                files,
                file_sizes,
                symlinks,
                modes,
                repo_root,
                pattern,
                src_root,
                heavy_rel,
                dest_root,
                lockfile_hash,
                verify,
                snapshots_enabled,
                v2_enabled,
            );
            SnapshotOutcome::FellBack(None)
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn try_lockfile_hit_impl(
    store: &mut DiskStore,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: &ContentId,
    verify: bool,
) -> SnapshotOutcome {
    if verify {
        return SnapshotOutcome::FellBack(None);
    }
    let backend = ClonefileBackend;
    if !backend.supports(dest_root) {
        return SnapshotOutcome::FellBack(None);
    }
    let repo_key = repo_root.to_string_lossy().into_owned();
    let idx = crate::SelectionIndex::load(store.root());
    let Some(rec) = idx
        .records
        .iter()
        .find(|r| r.matches(&repo_key, pattern, heavy_rel))
    else {
        return SnapshotOutcome::FellBack(None);
    };

    let heavy_src = src_root.join(heavy_rel);
    let Ok(src_meta) = fs::symlink_metadata(&heavy_src) else {
        return SnapshotOutcome::FellBack(None);
    };
    if !src_meta.is_dir() {
        return SnapshotOutcome::FellBack(None);
    }
    let src_mtime_secs = src_meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Root directory modification times must be unchanged since snapshot publish
    if rec.mtime_secs > 0 && src_mtime_secs > rec.mtime_secs + 1 {
        return SnapshotOutcome::FellBack(None);
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
                let tree = snapshot_tree_path(store.root(), hash);
                let stage = Instant::now();
                let cloned = finish_clone(&backend, &tree, &dest_heavy);
                clonefile_ms += stage.elapsed().as_millis();
                match cloned {
                    Ok(()) => {
                        let _ = crate::record_snapshot_hit(
                            store.root(),
                            &repo_key,
                            pattern,
                            heavy_rel,
                            hash,
                        );
                        let file_count = manifest
                            .entries
                            .iter()
                            .filter(|e| e.kind == crate::snapshot::EntryKind::File)
                            .count();
                        return SnapshotOutcome::Hydrated(SnapshotHydration {
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
                    Err(CloneFailure::Refused(diag)) => return SnapshotOutcome::FellBack(diag),
                    Err(CloneFailure::Fatal(msg)) => return SnapshotOutcome::Failed(msg),
                }
            }
        }
    }

    SnapshotOutcome::FellBack(None)
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn hydrate_impl(
    store: &mut DiskStore,
    dirs: &[String],
    files: &BTreeMap<String, ContentId>,
    file_sizes: &BTreeMap<String, u64>,
    symlinks: &BTreeMap<String, String>,
    modes: &BTreeMap<String, u32>,
    repo_root: &Path,
    pattern: &str,
    src_root: &Path,
    heavy_rel: &str,
    dest_root: &Path,
    lockfile_hash: Option<&ContentId>,
    verify: bool,
    snapshots_enabled: bool,
    v2_enabled: bool,
) -> SnapshotOutcome {
    if !snapshots_enabled {
        return SnapshotOutcome::FellBack(None);
    }
    let paranoid = verify;
    let backend = ClonefileBackend;
    // Gate is a no-op on filesystems without recursive clone support.
    if !backend.supports(dest_root) {
        return SnapshotOutcome::FellBack(None);
    }
    // v2 needs the same APFS substrate PLUS its own env gate.
    let v2 = snapshots_enabled && v2_enabled;
    let repo_key = repo_root.to_string_lossy().into_owned();

    let entries = match manifest_entries(dirs, files, symlinks, modes, heavy_rel) {
        Ok(entries) => entries,
        Err(msg) => return SnapshotOutcome::Failed(msg),
    };
    let mut unique_blobs = std::collections::BTreeMap::new();
    for (rel, id) in files {
        if let Some(&size) = file_sizes.get(rel) {
            unique_blobs.entry(*id).or_insert(size);
        }
    }
    let total_size: u64 = unique_blobs.values().sum();
    let manifest =
        match Manifest::new_with_lockfile_and_size(entries, lockfile_hash.copied(), total_size) {
            Ok(m) => m,
            Err(msg) => {
                return SnapshotOutcome::Failed(format!("cannot build snapshot manifest: {msg}"));
            }
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
                    let _ = crate::record_snapshot_hit(
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
                    crate::record_snapshot_lru_touch(store.root(), &manifest.hash);
                }
                let tree = snapshot_tree_path(store.root(), &manifest.hash);
                let stage = Instant::now();
                let cloned = finish_clone(&backend, &tree, &dest_heavy);
                clonefile_ms += stage.elapsed().as_millis();
                match cloned {
                    Ok(()) => {
                        return SnapshotOutcome::Hydrated(SnapshotHydration {
                            hash: manifest.hash,
                            files: files.len(),
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
                    Err(CloneFailure::Refused(diag)) => return SnapshotOutcome::FellBack(diag),
                    Err(CloneFailure::Fatal(msg)) => return SnapshotOutcome::Failed(msg),
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
            match ensure_published(store, files, src_root, &manifest, paranoid, &mut build) {
                Ok(PublishOutcome::Published | PublishOutcome::WinnerValid) => {}
                Ok(PublishOutcome::WinnerInvalid) => {
                    // Debris sits on our final name; never overwrite what
                    // we cannot prove ours. Per-file ladder keeps working.
                    return SnapshotOutcome::FellBack(Some(format!(
                        "snapshot {} exists but is invalid debris; using per-file placement",
                        manifest.hash
                    )));
                }
                Err(msg) => return SnapshotOutcome::Failed(msg),
            }
        }

        if v2 {
            // Every successful publish — full or incremental — moves
            // the hash to the front of the selection ring.
            let _ = crate::record_snapshot_publish(
                store.root(),
                &repo_key,
                pattern,
                heavy_rel,
                &manifest.hash,
            );
        } else {
            // Same recency rule as the hit path above.
            crate::record_snapshot_lru_touch(store.root(), &manifest.hash);
        }

        let stage = Instant::now();
        let cloned = finish_clone(
            &backend,
            &snapshot_tree_path(store.root(), &manifest.hash),
            &dest_heavy,
        );
        clonefile_ms += stage.elapsed().as_millis();
        match cloned {
            Ok(()) => {
                let (cloned_units, linked_files) = incremental.unwrap_or((0, 0));
                return SnapshotOutcome::Hydrated(SnapshotHydration {
                    hash: manifest.hash,
                    files: files.len(),
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
            Err(CloneFailure::Refused(diag)) => return SnapshotOutcome::FellBack(diag),
            Err(CloneFailure::Fatal(msg)) => return SnapshotOutcome::Failed(msg),
        }
    }
    SnapshotOutcome::Failed(format!(
        "snapshot {} kept vanishing between publish and clone",
        manifest.hash
    ))
}

/// The v2 attempt: pick an old snapshot from the selection index,
/// diff it against the new manifest, and rebuild incrementally when
/// that looks cheaper than a full build.
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

/// Clone the published tree to `dest_heavy`, honoring destination rules:
/// absent -> clone; empty-and-command-owned -> remove and clone;
/// non-empty -> merge via the per-file ladder.
#[cfg(target_os = "macos")]
fn finish_clone(
    backend: &ClonefileBackend,
    tree: &Path,
    dest_heavy: &Path,
) -> Result<(), CloneFailure> {
    let mut restore_empty_dir = false;
    match fs::symlink_metadata(dest_heavy) {
        Ok(md) if md.is_dir() => {
            let mut contents = fs::read_dir(dest_heavy).map_err(|e| {
                CloneFailure::Fatal(format!("cannot inspect {}: {e}", dest_heavy.display()))
            })?;
            if contents.next().is_some() {
                return Err(CloneFailure::Refused(None));
            }
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
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::SnapshotVanished)
        }
        Err(wt_copy::Error::DestinationExists) => Err(CloneFailure::Refused(None)),
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

/// Remove a partial destination tree this command created.
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
/// heavy directory itself.
#[cfg(target_os = "macos")]
fn manifest_entries(
    dirs: &[String],
    files: &BTreeMap<String, ContentId>,
    symlinks: &BTreeMap<String, String>,
    modes: &BTreeMap<String, u32>,
    heavy_rel: &str,
) -> Result<Vec<SnapshotEntry>, String> {
    let prefix = format!("{heavy_rel}/");
    let strip = |path: &str| -> Result<String, String> {
        path.strip_prefix(&prefix)
            .map(str::to_owned)
            .ok_or_else(|| format!("ingested path {path:?} lies outside {heavy_rel:?}"))
    };
    let mut out = Vec::new();
    for dir in dirs {
        if dir == heavy_rel {
            continue;
        }
        out.push(SnapshotEntry::dir(strip(dir)?));
    }
    for (path, id) in files {
        let rel = strip(path)?;
        let mode = modes.get(path).copied().unwrap_or(0o644);
        out.push(SnapshotEntry::file(rel, *id, mode));
    }
    for (path, target) in symlinks {
        out.push(SnapshotEntry::symlink(strip(path)?, target));
    }
    Ok(out)
}

/// Make sure a valid published snapshot exists for `manifest`,
/// building and publishing it when missing.
#[cfg(target_os = "macos")]
fn ensure_published(
    store: &mut DiskStore,
    files: &BTreeMap<String, ContentId>,
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
                healed = true;
                heal_blob(store, files, src_root, blob)?;
            }
            Err(BuildError::MissingBlob(blob)) => {
                return Err(format!("blob {blob} vanished twice during snapshot build"));
            }
            Err(BuildError::Fatal(msg)) => return Err(msg),
            Err(BuildError::Io(e)) => return Err(e.to_string()),
        }
    }
}

/// Re-store the source bytes behind `blob`.
#[cfg(target_os = "macos")]
fn heal_blob(
    store: &mut DiskStore,
    files: &BTreeMap<String, ContentId>,
    src_root: &Path,
    blob: ContentId,
) -> Result<(), String> {
    use crate::Store as _;

    let rel = files
        .iter()
        .find(|(_, id)| **id == blob)
        .map(|(rel, _)| rel.clone())
        .ok_or_else(|| format!("blob {blob} vanished but has no known source to heal from"))?;
    let bytes = fs::read(src_root.join(&rel))
        .map_err(|e| format!("cannot re-read {rel} for blob healing: {e}"))?;
    store.put(&bytes).map_err(|e| e.to_string())?;
    Ok(())
}
