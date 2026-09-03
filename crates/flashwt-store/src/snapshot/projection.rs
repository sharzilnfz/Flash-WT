use std::collections::BTreeMap;
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
use crate::snapshot::{BuildError, Manifest, PublishOptions, PublishOutcome, snapshot_tree_path};
use crate::{ContentId, DiskStore};
#[cfg(target_os = "macos")]
use flashwt_copy::{ClonefileBackend, CopyBackend};

pub const INCREMENTAL_DIFF_RATIO_MAX: f64 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IncrementalDecision {
    Hit,

    DiffTooWide,

    LockfileMiss,

    NoBaseSnapshot,

    EmptyManifest,

    PublishFailed,
}

impl IncrementalDecision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::DiffTooWide => "diff_too_wide",
            Self::LockfileMiss => "lockfile_miss",
            Self::NoBaseSnapshot => "no_base_snapshot",
            Self::EmptyManifest => "empty_manifest",
            Self::PublishFailed => "publish_failed",
        }
    }
}

impl std::fmt::Display for IncrementalDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug)]
pub enum IncrementalResult {
    Hit {
        cloned_units: usize,

        linked_files: usize,

        hit_rate: f64,
    },

    Fallback {
        decision: IncrementalDecision,

        reason: String,

        hit_rate: Option<f64>,
    },
}

#[derive(Debug, Clone)]
pub struct SnapshotHydration {
    pub hash: ContentId,

    pub files: usize,

    pub mode: &'static str,

    pub cloned_units: usize,

    pub linked_files: usize,

    pub lookup_ms: u128,

    pub clonefile_ms: u128,

    pub build: Option<SnapshotBuildTiming>,

    pub incremental_decision: Option<IncrementalDecision>,

    pub incremental_fallback_reason: Option<String>,

    pub incremental_hit_rate: Option<f64>,
}

#[derive(Debug)]
pub enum SnapshotOutcome {
    Hydrated(SnapshotHydration),

    FellBack(Option<String>),

    Failed(String),
}

#[derive(Debug, Clone)]
pub struct SnapshotProjectionRequest<'a> {
    pub dirs: &'a [String],

    pub dir_modes: &'a BTreeMap<String, u32>,

    pub files: &'a BTreeMap<String, ContentId>,

    pub file_sizes: &'a BTreeMap<String, u64>,

    pub symlinks: &'a BTreeMap<String, String>,

    pub modes: &'a BTreeMap<String, u32>,

    pub repo_root: &'a Path,

    pub pattern: &'a str,

    pub src_root: &'a Path,

    pub heavy_rel: &'a str,

    pub dest_root: &'a Path,

    pub lockfile_hash: Option<&'a ContentId>,

    pub verify: bool,

    pub snapshots_enabled: bool,

    pub v2_enabled: bool,
}

pub struct SnapshotProjectionEngine;

impl SnapshotProjectionEngine {
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

    pub fn hydrate(store: &mut DiskStore, req: &SnapshotProjectionRequest<'_>) -> SnapshotOutcome {
        #[cfg(target_os = "macos")]
        {
            hydrate_impl(store, req)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (store, req);
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

    if rec.mtime_secs > 0 && src_mtime_secs > rec.mtime_secs + 1 {
        return SnapshotOutcome::FellBack(None);
    }

    if rec.mtime_secs > 0 && is_nested_stale(&heavy_src, rec.mtime_secs) {
        return SnapshotOutcome::FellBack(Some(
            "nested file newer than snapshot; invalidating lockfile fast path".into(),
        ));
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
                            incremental_decision: None,
                            incremental_fallback_reason: None,
                            incremental_hit_rate: None,
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
fn is_nested_stale(heavy_src: &Path, snapshot_secs: u64) -> bool {
    if snapshot_secs == 0 {
        return false;
    }
    if let Ok(entries) = crate::bulkwalk::walk(heavy_src) {
        for e in entries {
            if e.mtime_secs > snapshot_secs {
                return true;
            }
        }
        return false;
    }
    portable_nested_stale(heavy_src, snapshot_secs)
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn is_nested_stale(heavy_src: &Path, snapshot_secs: u64) -> bool {
    portable_nested_stale(heavy_src, snapshot_secs)
}

#[allow(dead_code)]
fn portable_nested_stale(heavy_src: &Path, snapshot_secs: u64) -> bool {
    if snapshot_secs == 0 {
        return false;
    }
    let mut stack = vec![heavy_src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return true,
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return true,
            };
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => return true,
            };
            let mtime_secs = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if mtime_secs > snapshot_secs {
                return true;
            }
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(path);
            }
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn hydrate_impl(store: &mut DiskStore, req: &SnapshotProjectionRequest<'_>) -> SnapshotOutcome {
    if !req.snapshots_enabled {
        return SnapshotOutcome::FellBack(None);
    }
    let paranoid = req.verify;
    let backend = ClonefileBackend;

    if !backend.supports(req.dest_root) {
        return SnapshotOutcome::FellBack(None);
    }

    let v2 = req.snapshots_enabled && req.v2_enabled;
    let repo_key = req.repo_root.to_string_lossy().into_owned();

    let manifest = match crate::ingest::manifest_from_parts(
        req.dirs,
        req.dir_modes,
        req.files,
        req.file_sizes,
        req.symlinks,
        req.modes,
        req.heavy_rel,
        req.lockfile_hash.copied(),
    ) {
        Ok(m) => m,
        Err(msg) => {
            return SnapshotOutcome::Failed(format!("cannot build snapshot manifest: {msg}"));
        }
    };

    let dest_heavy = req.dest_root.join(req.heavy_rel);

    let mut lookup_ms = 0u128;
    let mut clonefile_ms = 0u128;
    let mut build: Option<SnapshotBuildTiming> = None;

    for attempt in 0..2u8 {
        if !paranoid && attempt == 0 {
            let stage = Instant::now();
            let published = store.find_snapshot(&manifest.hash);
            lookup_ms += stage.elapsed().as_millis();
            if published.is_some() {
                if v2 {
                    let _ = crate::record_snapshot_hit(
                        store.root(),
                        &repo_key,
                        req.pattern,
                        req.heavy_rel,
                        &manifest.hash,
                    );
                } else {
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
                            files: req.files.len(),
                            mode: "hit",
                            cloned_units: 0,
                            linked_files: 0,
                            lookup_ms,
                            clonefile_ms,
                            build,
                            incremental_decision: None,
                            incremental_fallback_reason: None,
                            incremental_hit_rate: None,
                        });
                    }

                    Err(CloneFailure::SnapshotVanished) => continue,
                    Err(CloneFailure::Refused(diag)) => return SnapshotOutcome::FellBack(diag),
                    Err(CloneFailure::Fatal(msg)) => return SnapshotOutcome::Failed(msg),
                }
            }
        }

        let mut incremental: Option<(usize, usize)> = None;
        let mut incremental_decision: Option<IncrementalDecision> = None;
        let mut incremental_fallback_reason: Option<String> = None;
        let mut incremental_hit_rate: Option<f64> = None;

        if v2 {
            match try_incremental(
                store,
                &manifest,
                &repo_key,
                req.pattern,
                req.heavy_rel,
                paranoid,
                &mut build,
            ) {
                IncrementalResult::Hit {
                    cloned_units,
                    linked_files,
                    hit_rate,
                } => {
                    incremental = Some((cloned_units, linked_files));
                    incremental_decision = Some(IncrementalDecision::Hit);
                    incremental_hit_rate = Some(hit_rate);
                }
                IncrementalResult::Fallback {
                    decision,
                    reason,
                    hit_rate,
                } => {
                    incremental_decision = Some(decision);
                    incremental_fallback_reason = Some(reason);
                    incremental_hit_rate = hit_rate;
                }
            }
        }

        if incremental.is_none() {
            match ensure_published(
                store,
                req.files,
                req.src_root,
                &manifest,
                paranoid,
                &mut build,
            ) {
                Ok(PublishOutcome::Published | PublishOutcome::WinnerValid) => {}
                Ok(PublishOutcome::WinnerInvalid) => {
                    return SnapshotOutcome::FellBack(Some(format!(
                        "snapshot {} exists but is invalid debris; using per-file placement",
                        manifest.hash
                    )));
                }
                Err(msg) => return SnapshotOutcome::Failed(msg),
            }
        }

        if v2 {
            let _ = crate::record_snapshot_publish(
                store.root(),
                &repo_key,
                req.pattern,
                req.heavy_rel,
                &manifest.hash,
            );
        } else {
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
                    files: req.files.len(),
                    mode: if incremental.is_some() { "v2" } else { "build" },
                    cloned_units,
                    linked_files,
                    lookup_ms,
                    clonefile_ms,
                    build,
                    incremental_decision,
                    incremental_fallback_reason,
                    incremental_hit_rate,
                });
            }

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

#[cfg(target_os = "macos")]
pub fn try_incremental(
    store: &mut DiskStore,
    manifest: &Manifest,
    repo_key: &str,
    pattern: &str,
    heavy_rel: &str,
    paranoid: bool,
    build: &mut Option<SnapshotBuildTiming>,
) -> IncrementalResult {
    let Some((old_hash, old_manifest)) =
        select_old_snapshot(store.root(), repo_key, pattern, heavy_rel)
    else {
        return IncrementalResult::Fallback {
            decision: IncrementalDecision::NoBaseSnapshot,
            reason: "no previous snapshot found in selection index".to_string(),
            hit_rate: None,
        };
    };

    let total = manifest.entries.len();
    if total == 0 {
        return IncrementalResult::Fallback {
            decision: IncrementalDecision::EmptyManifest,
            reason: "manifest has no entries".to_string(),
            hit_rate: None,
        };
    }

    if manifest.lockfile_hash != old_manifest.lockfile_hash {
        return IncrementalResult::Fallback {
            decision: IncrementalDecision::LockfileMiss,
            reason: "lockfile hash mismatch between current manifest and base snapshot".to_string(),
            hit_rate: None,
        };
    }

    let diff = SnapshotDiff::compute(&old_manifest.entries, &manifest.entries);
    let changed = diff.changed_count();
    let changed_ratio = changed as f64 / total as f64;
    let hit_rate = if total > 0 {
        (total.saturating_sub(changed)) as f64 / total as f64
    } else {
        1.0
    };
    if changed_ratio > INCREMENTAL_DIFF_RATIO_MAX {
        return IncrementalResult::Fallback {
            decision: IncrementalDecision::DiffTooWide,
            reason: format!(
                "changed entries ratio {:.2}% ({} of {}) exceeds maximum threshold {:.2}%",
                changed_ratio * 100.0,
                changed,
                total,
                INCREMENTAL_DIFF_RATIO_MAX * 100.0,
            ),
            hit_rate: Some(hit_rate),
        };
    }

    let opts = PublishOptions::default()
        .lockfile_hash(manifest.lockfile_hash)
        .base_snapshot(Some(old_hash))
        .paranoid(paranoid);
    match store.publish_manifest(manifest, opts) {
        Ok(receipt) => {
            let timing = receipt.timing;
            *build = Some(timing);
            IncrementalResult::Hit {
                cloned_units: timing.clone_units,
                linked_files: timing.linked_files,
                hit_rate,
            }
        }
        Err(e) => IncrementalResult::Fallback {
            decision: IncrementalDecision::PublishFailed,
            reason: format!("incremental publish failed: {e}"),
            hit_rate: None,
        },
    }
}

#[cfg(not(target_os = "macos"))]
pub fn try_incremental(
    _store: &mut DiskStore,
    _manifest: &Manifest,
    _repo_key: &str,
    _pattern: &str,
    _heavy_rel: &str,
    _paranoid: bool,
    _build: &mut Option<SnapshotBuildTiming>,
) -> IncrementalResult {
    IncrementalResult::Fallback {
        decision: IncrementalDecision::PublishFailed,
        reason: "incremental snapshots unsupported on this platform".to_string(),
        hit_rate: None,
    }
}

#[cfg(target_os = "macos")]
enum CloneFailure {
    SnapshotVanished,

    Refused(Option<String>),

    Fatal(String),
}

#[cfg(target_os = "macos")]
fn finish_clone(
    backend: &ClonefileBackend,
    tree: &Path,
    dest_heavy: &Path,
) -> Result<(), CloneFailure> {
    if let Some(parent) = dest_heavy.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            CloneFailure::Fatal(format!(
                "cannot create parent dirs for {}: {e}",
                dest_heavy.display()
            ))
        })?;
    }

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
        Err(flashwt_copy::Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::SnapshotVanished)
        }
        Err(flashwt_copy::Error::DestinationExists) => Err(CloneFailure::Refused(None)),
        Err(flashwt_copy::Error::Unsupported) => {
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::Refused(None))
        }
        Err(flashwt_copy::Error::Io(e)) => {
            cleanup_partial(dest_heavy, restore_empty_dir)?;
            Err(CloneFailure::Fatal(format!(
                "clonefile {} -> {}: {e}",
                tree.display(),
                dest_heavy.display()
            )))
        }
    }
}

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
        let opts = PublishOptions::default()
            .lockfile_hash(manifest.lockfile_hash)
            .paranoid(paranoid);
        match store.publish_manifest(manifest, opts) {
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

#[cfg(target_os = "macos")]
fn heal_blob(
    store: &mut DiskStore,
    files: &BTreeMap<String, ContentId>,
    src_root: &Path,
    blob: ContentId,
) -> Result<(), String> {
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
