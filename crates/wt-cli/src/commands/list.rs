//! `wt list` / `wt ls`: Worktree discovery and disk space accounting (ticket 02).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use wt_store::{
    ContentId, DiskStore, EntryKind, StoreMirror, is_lease_expired, is_process_alive, mirror_path,
    read_leases, read_published_snapshot,
};

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, LeaseEntry, ListData, WorktreeEntry};
use crate::error::Result;
use crate::gitops;
use crate::hydrate::open_store;

/// Raw git worktree information parsed from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawGitWorktree {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_detached: bool,
    pub is_bare: bool,
    pub is_locked: bool,
    pub is_prunable: bool,
}

/// Parse the output of `git worktree list --porcelain`.
pub fn parse_git_worktrees(porcelain_output: &str) -> Vec<RawGitWorktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_detached = false;
    let mut is_bare = false;
    let mut is_locked = false;
    let mut is_prunable = false;

    let flush = |worktrees: &mut Vec<RawGitWorktree>,
                 path: &mut Option<PathBuf>,
                 head: &mut Option<String>,
                 branch: &mut Option<String>,
                 detached: &mut bool,
                 bare: &mut bool,
                 locked: &mut bool,
                 prunable: &mut bool| {
        if let Some(p) = path.take() {
            worktrees.push(RawGitWorktree {
                path: p,
                head: head.take(),
                branch: branch.take(),
                is_detached: *detached,
                is_bare: *bare,
                is_locked: *locked,
                is_prunable: *prunable,
            });
            *detached = false;
            *bare = false;
            *locked = false;
            *prunable = false;
        }
    };

    for line in porcelain_output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(
                &mut worktrees,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
                &mut is_detached,
                &mut is_bare,
                &mut is_locked,
                &mut is_prunable,
            );
            continue;
        }
        if let Some(path_str) = trimmed.strip_prefix("worktree ") {
            flush(
                &mut worktrees,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
                &mut is_detached,
                &mut is_bare,
                &mut is_locked,
                &mut is_prunable,
            );
            current_path = Some(PathBuf::from(path_str.trim()));
        } else if let Some(h) = trimmed.strip_prefix("HEAD ") {
            current_head = Some(h.trim().to_string());
        } else if let Some(b) = trimmed.strip_prefix("branch ") {
            let branch_ref = b.trim();
            let branch_name = branch_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(branch_ref);
            current_branch = Some(branch_name.to_string());
        } else if trimmed == "detached" {
            is_detached = true;
        } else if trimmed == "bare" {
            is_bare = true;
        } else if trimmed.starts_with("locked") {
            is_locked = true;
        } else if trimmed.starts_with("prunable") {
            is_prunable = true;
        }
    }
    flush(
        &mut worktrees,
        &mut current_path,
        &mut current_head,
        &mut current_branch,
        &mut is_detached,
        &mut is_bare,
        &mut is_locked,
        &mut is_prunable,
    );

    worktrees
}

/// Resolve the git directory (`git_dir`) for a worktree path.
fn resolve_git_dir(worktree_path: &Path) -> PathBuf {
    let dot_git = worktree_path.join(".git");
    if dot_git.is_dir() {
        return dot_git;
    }
    if dot_git.is_file() {
        if let Ok(content) = fs::read_to_string(&dot_git) {
            for line in content.lines() {
                if let Some(rest) = line.trim().strip_prefix("gitdir:") {
                    let gitdir_path = PathBuf::from(rest.trim());
                    if gitdir_path.is_absolute() {
                        return gitdir_path;
                    } else {
                        return worktree_path.join(gitdir_path);
                    }
                }
            }
        }
    }
    if let Ok(dir_str) = gitops::run(worktree_path, &["rev-parse", "--absolute-git-dir"]) {
        return PathBuf::from(dir_str);
    }
    dot_git
}

/// Format bytes into a human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if bytes == 0 {
        "0 B".to_string()
    } else if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.1} GB", b / GB)
    }
}

/// Format duration into a human-readable string (e.g. 45s, 12m, 3h, 2d).
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Lookup blob size in store, using an in-memory cache to avoid repeated stats.
fn get_blob_size(
    id: &ContentId,
    store: &DiskStore,
    cache: &mut BTreeMap<ContentId, u64>,
) -> u64 {
    if let Some(&size) = cache.get(id) {
        return size;
    }
    let blob_path = store.blob_path(id);
    let size = fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);
    cache.insert(*id, size);
    size
}

pub fn run(cfg: &RunConfig) -> Result<(ListData, Vec<Diagnostic>)> {
    let repo_root = gitops::repo_root()?;
    let canon_repo_root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.clone());
    let current_dir = std::env::current_dir().ok();
    let canon_cwd = current_dir.as_ref().and_then(|d| d.canonicalize().ok());

    let raw_out = gitops::run(&repo_root, &["worktree", "list", "--porcelain"])?;
    let raw_worktrees = parse_git_worktrees(&raw_out);

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
        let worktree_path = raw.path.clone();
        let canon_worktree = worktree_path
            .canonicalize()
            .unwrap_or_else(|_| worktree_path.clone());
        let git_dir = resolve_git_dir(&worktree_path);
        let canon_gitdir = git_dir.canonicalize().unwrap_or_else(|_| git_dir.clone());

        let branch_display = if let Some(ref b) = raw.branch {
            b.clone()
        } else if raw.is_detached {
            "(detached)".to_string()
        } else if raw.is_bare {
            "(bare)".to_string()
        } else {
            "(unknown)".to_string()
        };

        // Determine if active
        let is_active = if let Some(ref cwd) = canon_cwd {
            cwd == &canon_worktree || cwd.starts_with(&canon_worktree)
        } else {
            false
        };

        // Determine if main worktree
        let is_main = canon_worktree == canon_repo_root || worktree_path.join(".git").is_dir();

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
                let match_by_path = lease.worktree == canon_worktree || lease.gitdir == canon_gitdir;
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
            head: raw.head,
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

    let mut rows: Vec<(String, String, String, String, String, String)> = Vec::new();

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
                format_bytes(entry.bytes_hydrated)
            )
        };
        let disk_saved = format_bytes(entry.bytes_saved);
        let status = if let Some(ref l) = entry.lease {
            if l.is_expired {
                format!("expired (pid: {})", l.pid)
            } else if l.pid_alive {
                format!("ttl: {} (pid: {})", format_duration(l.ttl_remaining_secs), l.pid)
            } else {
                format!("ttl: {} (pid: {} [dead])", format_duration(l.ttl_remaining_secs), l.pid)
            }
        } else if let Some(age) = entry.age_secs {
            format!("{} ago", format_duration(age))
        } else {
            "-".to_string()
        };

        rows.push((
            active_mark.to_string(),
            branch.clone(),
            path.clone(),
            hydrated,
            disk_saved,
            status,
        ));
    }

    let max_branch = rows.iter().map(|r| r.1.len()).max().unwrap_or(6).max(6); // "BRANCH".len() == 6
    let max_path = rows.iter().map(|r| r.2.len()).max().unwrap_or(4).max(4); // "PATH".len() == 4
    let max_hydrated = rows.iter().map(|r| r.3.len()).max().unwrap_or(8).max(8); // "HYDRATED".len() == 8
    let max_saved = rows.iter().map(|r| r.4.len()).max().unwrap_or(10).max(10); // "DISK SAVED".len() == 10

    println!(
        "  {:<max_branch$}  {:<max_path$}  {:<max_hydrated$}  {:<max_saved$}  AGE / STATUS",
        "BRANCH", "PATH", "HYDRATED", "DISK SAVED"
    );

    for (active, branch, path, hydrated, saved, status) in rows {
        println!(
            "{active} {:<max_branch$}  {:<max_path$}  {:<max_hydrated$}  {:<max_saved$}  {status}",
            branch, path, hydrated, saved
        );
    }

    let worktree_count = entries.len();
    let total_files: usize = entries.iter().map(|e| e.files_hydrated).sum();

    println!();
    if total_files > 0 {
        println!(
            "Total disk saved: {} across {} worktree{} ({} files deduplicated)",
            format_bytes(total_disk_saved),
            worktree_count,
            if worktree_count == 1 { "" } else { "s" },
            total_files
        );
    } else {
        println!(
            "Total disk saved: {} across {} worktree{}",
            format_bytes(total_disk_saved),
            worktree_count,
            if worktree_count == 1 { "" } else { "s" }
        );
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_porcelain_worktrees_multiple_entries() {
        let output = "\
worktree /path/to/main
HEAD 94a106886e0fe0d3810d7ceb10c9502844281315
branch refs/heads/main

worktree /path/to/feature-1
HEAD 1234567890abcdef1234567890abcdef12345678
branch refs/heads/feature-1

worktree /path/to/detached-wt
HEAD fedcba0987654321fedcba0987654321fedcba09
detached

worktree /path/to/bare-wt
bare
";
        let parsed = parse_git_worktrees(output);
        assert_eq!(parsed.len(), 4);

        assert_eq!(parsed[0].path, PathBuf::from("/path/to/main"));
        assert_eq!(
            parsed[0].head.as_deref(),
            Some("94a106886e0fe0d3810d7ceb10c9502844281315")
        );
        assert_eq!(parsed[0].branch.as_deref(), Some("main"));
        assert!(!parsed[0].is_detached);
        assert!(!parsed[0].is_bare);

        assert_eq!(parsed[1].path, PathBuf::from("/path/to/feature-1"));
        assert_eq!(parsed[1].branch.as_deref(), Some("feature-1"));

        assert_eq!(parsed[2].path, PathBuf::from("/path/to/detached-wt"));
        assert!(parsed[2].is_detached);
        assert_eq!(parsed[2].branch, None);

        assert_eq!(parsed[3].path, PathBuf::from("/path/to/bare-wt"));
        assert!(parsed[3].is_bare);
    }

    #[test]
    fn format_bytes_examples() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn format_duration_examples() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(120), "2m");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(86400 * 3), "3d");
    }
}
