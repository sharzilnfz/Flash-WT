//! `wt list` / `wt ls`: Worktree discovery and disk space accounting (ticket 02).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use wt_store::{
    ContentId, DiskStore, EntryKind, StoreMirror, is_lease_expired, is_process_alive, mirror_path,
    read_leases, read_published_snapshot,
};

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, LeaseEntry, ListData, WorktreeEntry};
use crate::error::Result;
use crate::hydrate::open_store;
use crate::output::{HumanBytes, HumanDuration, format_table};
use crate::workspace::WorkspaceEngine;

/// Lookup blob size in store, using an in-memory cache to avoid repeated stats.
fn get_blob_size(id: &ContentId, store: &DiskStore, cache: &mut BTreeMap<ContentId, u64>) -> u64 {
    if let Some(&size) = cache.get(id) {
        return size;
    }
    let blob_path = store.blob_path(id);
    let size = fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);
    cache.insert(*id, size);
    size
}

pub fn run(cfg: &RunConfig) -> Result<(ListData, Vec<Diagnostic>)> {
    let engine = WorkspaceEngine::discover()?;

    let raw_worktrees = engine.worktrees()?;

    let store = open_store()?;
    let store_root = store.root().to_path_buf();
    let read_leases_list = read_leases(&store_root);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut blob_size_cache = BTreeMap::new();
    let mut entries = Vec::new();
    let mut total_disk_saved = 0u64;
    let mut total_files_hydrated = 0usize;

    for raw in raw_worktrees {
        let md = engine.metadata(raw);
        let worktree_path = md.raw.path.clone();
        let canon_worktree = worktree_path
            .canonicalize()
            .unwrap_or_else(|_| worktree_path.clone());
        let git_dir = md.git_dir.clone();
        let canon_gitdir = git_dir.canonicalize().unwrap_or_else(|_| git_dir.clone());

        let branch_display = md.branch_display.clone();

        // Determine if active
        let is_active = engine.is_active(&worktree_path);

        // Determine if main worktree
        let is_main = md.is_main;

        // 1. Read wt-hydrated.tsv sidecar if present
        let mut sidecar_files: Vec<(String, ContentId)> = Vec::new();
        let mut sidecar_snapshots: Vec<(String, ContentId)> = Vec::new();
        let sidecar_path = git_dir.join("wt-hydrated.tsv");
        if sidecar_path.exists() {
            if let Ok(text) = fs::read_to_string(&sidecar_path) {
                for line in text.lines() {
                    let fields: Vec<&str> = line.split('\t').collect();
                    match fields.as_slice() {
                        [rel, hex] => {
                            if let Some(id) = ContentId::from_hex(hex) {
                                sidecar_files.push((rel.to_string(), id));
                            }
                        }
                        [rel, kind, hex] => match *kind {
                            "blob" => {
                                if let Some(id) = ContentId::from_hex(hex) {
                                    sidecar_files.push((rel.to_string(), id));
                                }
                            }
                            "snapshot" => {
                                if let Some(id) = ContentId::from_hex(hex) {
                                    sidecar_snapshots.push((rel.to_string(), id));
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }

        // 2. Read store mirror if present
        let mut mirror_base_branch = None;
        let mut mirror_files = BTreeSet::new();
        let mut mirror_snapshots = BTreeSet::new();
        let mirror_p = mirror_path(&store_root, &canon_worktree, &canon_gitdir);
        if mirror_p.exists() {
            if let Ok(text) = fs::read_to_string(&mirror_p) {
                if let Ok(mirror) = StoreMirror::parse(&text) {
                    mirror_base_branch = mirror.base_branch;
                    mirror_files = mirror.files;
                    mirror_snapshots = mirror.snapshots;
                }
            }
        }

        // 3. Compute hydrated files, directories, and disk space savings
        let mut files_hydrated = 0usize;
        let mut bytes_saved = 0u64;
        let mut hydrated_dirs_set = BTreeSet::new();

        if !sidecar_files.is_empty() {
            let mut seen_paths = std::collections::HashSet::new();
            for (rel, blob_id) in &sidecar_files {
                if seen_paths.insert(rel.clone()) {
                    files_hydrated += 1;
                    let sz = get_blob_size(blob_id, &store, &mut blob_size_cache);
                    bytes_saved += sz;
                    if let Some(first_comp) = rel.split('/').next() {
                        if !first_comp.is_empty() && first_comp != "-" {
                            hydrated_dirs_set.insert(first_comp.to_string());
                        }
                    }
                }
            }
        } else if !sidecar_snapshots.is_empty() {
            let mut seen_paths = std::collections::HashSet::new();
            for (_snap_mount, snap_id) in &sidecar_snapshots {
                if let Some(manifest) = read_published_snapshot(&store_root, snap_id) {
                    for entry in &manifest.entries {
                        if entry.kind == EntryKind::File && seen_paths.insert(entry.rel.clone()) {
                            files_hydrated += 1;
                            if let Some(blob_id) = entry.blob {
                                let sz = get_blob_size(&blob_id, &store, &mut blob_size_cache);
                                bytes_saved += sz;
                            }
                            if let Some(first_comp) = entry.rel.split('/').next() {
                                if !first_comp.is_empty() && first_comp != "-" {
                                    hydrated_dirs_set.insert(first_comp.to_string());
                                }
                            }
                        }
                    }
                }
            }
        } else if !mirror_files.is_empty() {
            for blob_id in &mirror_files {
                files_hydrated += 1;
                let sz = get_blob_size(blob_id, &store, &mut blob_size_cache);
                bytes_saved += sz;
            }
        } else if !mirror_snapshots.is_empty() {
            let mut seen_paths = std::collections::HashSet::new();
            for snap_id in &mirror_snapshots {
                if let Some(manifest) = read_published_snapshot(&store_root, snap_id) {
                    for entry in &manifest.entries {
                        if entry.kind == EntryKind::File && seen_paths.insert(entry.rel.clone()) {
                            files_hydrated += 1;
                            if let Some(blob_id) = entry.blob {
                                let sz = get_blob_size(&blob_id, &store, &mut blob_size_cache);
                                bytes_saved += sz;
                            }
                            if let Some(first_comp) = entry.rel.split('/').next() {
                                if !first_comp.is_empty() && first_comp != "-" {
                                    hydrated_dirs_set.insert(first_comp.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        let hydrated_dirs = if hydrated_dirs_set.is_empty() {
            None
        } else {
            Some(hydrated_dirs_set.into_iter().collect())
        };

        // 4. Match ephemeral scratch lease
        let mut lease_entry = None;
        let mut is_ephemeral = false;
        for read_lease in &read_leases_list {
            if let Ok(ref lease) = read_lease.lease {
                let match_by_path =
                    lease.worktree == canon_worktree || lease.gitdir == canon_gitdir;
                let match_by_name = lease.id == branch_display
                    || format!("scratch-{}", lease.id) == branch_display
                    || branch_display.strip_prefix("scratch-") == Some(&lease.id);

                if match_by_path || match_by_name {
                    is_ephemeral = true;
                    let pid_alive = is_process_alive(lease.pid, lease.start_time);
                    let expired = is_lease_expired(lease);
                    let ttl_remaining = lease.expires_at.saturating_sub(now_secs);
                    lease_entry = Some(LeaseEntry {
                        lease_id: lease.id.clone(),
                        pid: lease.pid,
                        pid_alive,
                        expires_at: lease.expires_at,
                        ttl_remaining_secs: ttl_remaining,
                        is_expired: expired,
                    });
                    break;
                }
            }
        }

        // 5. Compute age in seconds
        let age_secs = fs::metadata(&worktree_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .map(|d| d.as_secs());

        total_disk_saved += bytes_saved;
        total_files_hydrated += files_hydrated;

        entries.push(WorktreeEntry {
            path: worktree_path.display().to_string(),
            branch: branch_display,
            head: md.raw.head.clone(),
            is_active,
            is_main,
            is_ephemeral,
            files_hydrated,
            bytes_hydrated: bytes_saved,
            bytes_saved,
            hydrated_dirs,
            base_branch: mirror_base_branch,
            lease: lease_entry,
            age_secs,
        });
    }

    if !cfg.json {
        print_human_table(&entries, total_disk_saved);
    }

    let data = ListData {
        worktrees: entries,
        total_disk_saved,
        total_files_hydrated,
    };

    Ok((data, Vec::new()))
}

/// Print formatted human-readable aligned table.
fn print_human_table(entries: &[WorktreeEntry], total_disk_saved: u64) {
    if entries.is_empty() {
        println!("No active worktrees found.");
        return;
    }

    let mut rows: Vec<Vec<String>> = Vec::new();

    for entry in entries {
        let active_mark = if entry.is_active { "*" } else { " " };
        let branch = &entry.branch;
        let path = &entry.path;
        let hydrated = if entry.files_hydrated == 0 {
            "-".to_string()
        } else {
            format!(
                "{} files ({})",
                entry.files_hydrated,
                HumanBytes(entry.bytes_hydrated)
            )
        };
        let disk_saved = HumanBytes(entry.bytes_saved).to_string();
        let status = if let Some(ref l) = entry.lease {
            if l.is_expired {
                format!("expired (pid: {})", l.pid)
            } else if l.pid_alive {
                format!(
                    "ttl: {} (pid: {})",
                    HumanDuration(l.ttl_remaining_secs),
                    l.pid
                )
            } else {
                format!(
                    "ttl: {} (pid: {} [dead])",
                    HumanDuration(l.ttl_remaining_secs),
                    l.pid
                )
            }
        } else if let Some(age) = entry.age_secs {
            format!("{} ago", HumanDuration(age))
        } else {
            "-".to_string()
        };

        rows.push(vec![
            active_mark.to_string(),
            branch.clone(),
            path.clone(),
            hydrated,
            disk_saved,
            status,
        ]);
    }

    println!(
        "{}",
        format_table(
            &[
                "",
                "BRANCH",
                "PATH",
                "HYDRATED",
                "DISK SAVED",
                "AGE / STATUS"
            ],
            &rows
        )
    );

    let worktree_count = entries.len();
    let total_files: usize = entries.iter().map(|e| e.files_hydrated).sum();

    println!();
    if total_files > 0 {
        println!(
            "Total disk saved: {} across {} worktree{} ({} files deduplicated, estimated logical reuse)",
            HumanBytes(total_disk_saved),
            worktree_count,
            if worktree_count == 1 { "" } else { "s" },
            total_files
        );
    } else {
        println!(
            "Total disk saved: {} across {} worktree{} (estimated logical reuse)",
            HumanBytes(total_disk_saved),
            worktree_count,
            if worktree_count == 1 { "" } else { "s" }
        );
    }
}
