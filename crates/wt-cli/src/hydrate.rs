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
//! Integrity: materialize verifies each blob through `Store::get`
//! BEFORE anything is placed, so corrupt store content fails loudly
//! instead of landing bad bytes in a fresh tree (spec: silent
//! corruption is detectable).
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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use wt_copy::{CloneOut, FileMaterialize, HardlinkOut, placement_refused};
use wt_store::{ContentId, DiskStore, Store};

/// Where the per-machine store lives. `$WT_STORE` wins (tests use it
/// for isolation); otherwise XDG cache conventions.
fn store_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("WT_STORE") {
        return Ok(PathBuf::from(dir));
    }
    let base = match std::env::var_os("XDG_CACHE_HOME") {
        Some(xdg) => PathBuf::from(xdg),
        None => {
            let home =
                std::env::var_os("HOME").ok_or("cannot locate a home directory for the store")?;
            PathBuf::from(home).join(".cache")
        }
    };
    Ok(base.join("wt").join("store"))
}

pub fn open_store() -> Result<DiskStore, String> {
    let dir = store_dir()?;
    DiskStore::open(&dir).map_err(|e| format!("cannot open store at {}: {e}", dir.display()))
}

/// Everything one heavy directory contributed to the store.
pub struct Ingested {
    /// Repo-relative directory paths to recreate even when empty.
    pub dirs: Vec<String>,
    /// Repo-relative file path -> stored content address.
    pub files: BTreeMap<String, ContentId>,
}

/// Walk `src`, storing every regular file's bytes. Symlinks are never
/// followed out of `src`; they are skipped for now rather than
/// misinterpreted (matching the copy-backend trait's stance).
pub fn ingest_dir(store: &mut DiskStore, src_root: &Path, src: &Path) -> Result<Ingested, String> {
    let mut ingested = Ingested {
        dirs: Vec::new(),
        files: BTreeMap::new(),
    };
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        ingested.dirs.push(rel_text(src_root, &dir));
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| format!("cannot stat {}: {e}", path.display()))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let bytes =
                fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let id = store.put(&bytes).map_err(|e| e.to_string())?;
            ingested.files.insert(rel_text(src_root, &path), id);
        }
    }
    ingested.dirs.sort();
    ingested.dirs.dedup();
    Ok(ingested)
}

fn rel_text(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("path under ingestion root")
        .to_string_lossy()
        .into_owned()
}

/// Outcome of materializing one heavy directory.
pub struct MaterializeReport {
    /// Total files placed.
    pub files: usize,
    /// Files written as plain byte copies because the selected
    /// strategy was disabled or refused by the filesystem.
    pub copied: usize,
    /// Strategy attempted for this directory: `"copy-on-write"`
    /// (default), `"hardlink"` (`WT_HARDLINK=1`), or `"byte-copy"`
    /// (forced by `WT_NO_HARDLINK`, and the answer on platforms with
    /// no clone support yet). Check `copied` to see how much of it
    /// actually happened.
    pub strategy: &'static str,
}

/// Which placement strategy this run uses, from the environment:
///
/// - `WT_NO_HARDLINK` present: forced byte copies (the escape hatch).
///   Kept for compatibility with the ticket 07 flag.
/// - `WT_HARDLINK=1`: experimental hardlinked materialization —
///   maximum space sharing, but linked inodes are made read-only, so
///   tools that rewrite files in place fail loudly.
/// - otherwise: per-file CoW clones on macOS; byte copies elsewhere
///   until Linux reflink is validated for store materialization.
fn select_strategy() -> Option<Box<dyn FileMaterialize>> {
    if std::env::var_os("WT_NO_HARDLINK").is_some() {
        return None;
    }
    if std::env::var_os("WT_HARDLINK").is_some() {
        return Some(Box::new(HardlinkOut));
    }
    #[cfg(target_os = "macos")]
    {
        Some(Box::new(CloneOut))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Recreate the ingested tree under `dest_root` from store content.
///
/// Per file: verify first (`Store::get` reads and hashes every byte
/// and aborts loudly on mismatch), and only then place anything — a
/// corrupt blob never lands in a fresh tree. Placement tries the
/// selected strategy (CoW clone by default) against the verified
/// blob; filesystem refusals fall back silently to a byte copy of
/// those same verified bytes. Permission problems on the destination
/// are real failures and stay loud.
pub fn materialize(
    store: &DiskStore,
    ingested: &Ingested,
    dest_root: &Path,
) -> Result<MaterializeReport, String> {
    let backend = select_strategy();
    let mut copied = 0usize;
    for rel in &ingested.dirs {
        fs::create_dir_all(dest_root.join(rel))
            .map_err(|e| format!("cannot prepare {}: {e}", dest_root.join(rel).display()))?;
    }
    for (rel, id) in &ingested.files {
        // Hash verification happens here, before any placement: with
        // fclonefileat the kernel copies bytes directly from the blob
        // at clone time, so the blob must already be known-good or
        // corruption would land. The TOCTOU window between this check
        // and the kernel's read is the one the previous design had in
        // reverse; nothing else guards it today either.
        let bytes = store
            .get(id)
            .map_err(|e| format!("materialize {rel}: {e}"))?;
        let dest = dest_root.join(rel);
        let parent = dest
            .parent()
            .ok_or_else(|| format!("{rel} has no parent"))?;
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot prepare {}: {e}", parent.display()))?;

        let mut placed = false;
        if let Some(backend) = &backend {
            match backend.materialize_file(&store.blob_path(id), &dest) {
                Ok(()) => placed = true,
                Err(e) if placement_refused(&e) => {}
                Err(e) => return Err(format!("materialize {rel}: {e}")),
            }
        }
        if !placed {
            copied += 1;
            fs::write(&dest, &bytes)
                .map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
        }
    }
    Ok(MaterializeReport {
        files: ingested.files.len(),
        copied,
        strategy: backend
            .as_ref()
            .map(|b| b.name())
            .unwrap_or("byte-copy"),
    })
}

// Placement refusals ("this filesystem cannot do that") are
// classified by `wt_copy::placement_refused`; everything else is a
// real failure.

/// Give this worktree one reference on every distinct blob it uses,
/// then record the mapping where ticket 06 can find it.
pub fn claim_references(
    store: &mut DiskStore,
    worktree: &Path,
    ingested: &Ingested,
) -> Result<(), String> {
    let distinct: BTreeSet<&ContentId> = ingested.files.values().collect();
    for id in distinct {
        store.add_ref(id).map_err(|e| e.to_string())?;
    }

    let git_dir = Command::new("git")
        .current_dir(worktree)
        .args(["rev-parse", "--git-dir"])
        .output()
        .map_err(|e| format!("cannot query git dir: {e}"))?;
    if !git_dir.status.success() {
        return Err("newly created worktree is not a git worktree".into());
    }
    let git_dir = worktree.join(String::from_utf8_lossy(&git_dir.stdout).trim());
    let mut sidecar = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(git_dir.join("wt-hydrated.tsv"))
        .map_err(|e| format!("cannot open hydration ledger: {e}"))?;
    for (rel, id) in &ingested.files {
        writeln!(sidecar, "{rel}\t{id}").map_err(|e| format!("cannot write ledger: {e}"))?;
    }
    Ok(())
}
