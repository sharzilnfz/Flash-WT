//! `wt scratch` and `wt isolate`: Ephemeral sandboxes with lease persistence (ticket 03).

use std::fs;
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use wt_store::{
    DEFAULT_LEASE_TTL_SECS, WorktreeLease, current_process_start_time, publish_lease, remove_lease,
};

use crate::commands::create;
use crate::config::RunConfig;
use crate::envelope::{Diagnostic, ScratchData};
use crate::error::{Error, Result};
use crate::hydrate::open_store;
use crate::signal;
use crate::workspace;

/// Generate a unique 8-character hex id for scratch worktrees.
fn generate_scratch_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_nanos().hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    count.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{:08x}", hash as u32)
}

/// RAII cleanup guard for ephemeral worktrees.
pub struct ScratchGuard {
    pub name: String,
    pub worktree_path: PathBuf,
    pub lease_id: String,
    pub store_root: PathBuf,
    pub repo_root: PathBuf,
    pub active: bool,
}

impl ScratchGuard {
    pub fn cleanup(&mut self, cfg: &RunConfig) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        signal::clear_scratch();

        // 1. Remove the lease file
        let _ = remove_lease(&self.store_root, &self.lease_id);

        // 2. Perform worktree removal and reference release
        let _ = crate::gc::remove(&self.name, Some(&self.worktree_path), cfg);

        // 3. Delete the temporary git branch if it still exists
        let _ = workspace::run(&self.repo_root, &["branch", "-D", &self.name]);

        // 4. Clean up any leftover directory on disk
        if self.worktree_path.exists() {
            let _ = fs::remove_dir_all(&self.worktree_path);
        }

        Ok(())
    }

    pub fn disarm(&mut self) {
        self.active = false;
        signal::clear_scratch();
    }
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        if self.active {
            let cfg = RunConfig::from_env();
            let _ = self.cleanup(&cfg);
        }
    }
}

pub fn run(
    name: Option<&str>,
    manifest: Option<&Path>,
    dir: Option<&Path>,
    run_cmd: Option<&str>,
    ttl: Option<Duration>,
    cfg: &RunConfig,
) -> Result<(ScratchData, Vec<Diagnostic>, Option<i32>)> {
    let root = workspace::repo_root()?;
    let started = Instant::now();

    // Determine branch name and lease id
    let (branch_name, lease_id) = match name {
        Some(n) => (
            n.to_string(),
            n.strip_prefix("scratch-").unwrap_or(n).to_string(),
        ),
        None => {
            let id = generate_scratch_id();
            (format!("scratch-{id}"), id)
        }
    };

    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => workspace::default_worktree_dest(&root, &branch_name)?,
    };

    // 1. Create the worktree and hydrate
    let (create_data, diags) = create::run(&branch_name, None, manifest, Some(&dest), cfg)?;

    // 2. Resolve git dir and open store
    let git_dir = workspace::git_dir(&dest)?;
    let store = open_store()?;
    let store_root = store.root().to_path_buf();

    // 3. Compute TTL and expiration
    let ttl_secs = ttl
        .map(|d| d.as_secs())
        .or_else(|| {
            std::env::var("WT_LEASE_TTL")
                .ok()
                .and_then(|s| crate::gc::parse_age(&s).map(|d| d.as_secs()))
        })
        .unwrap_or(DEFAULT_LEASE_TTL_SECS);

    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expires_at = now_secs.saturating_add(ttl_secs);

    let pid = if let Some(owner) = std::env::var("WT_OWNER_PID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        owner
    } else if run_cmd.is_some() {
        std::process::id()
    } else {
        let ppid = unsafe { libc::getppid() } as u32;
        if ppid > 1 { ppid } else { std::process::id() }
    };
    let start_time = wt_store::process_start_time(pid).unwrap_or_else(current_process_start_time);
    let canon_worktree = dest.canonicalize().unwrap_or_else(|_| dest.clone());
    let canon_gitdir = git_dir.canonicalize().unwrap_or_else(|_| git_dir.clone());

    let lease = WorktreeLease::new(
        &lease_id,
        canon_worktree,
        canon_gitdir,
        pid,
        start_time,
        expires_at,
    );
    let lease_file_path = publish_lease(&store_root, &lease).map_err(|e| {
        Error::io(
            "write lease file",
            wt_store::lease_path(&store_root, &lease_id),
            e,
        )
    })?;

    let mut guard = ScratchGuard {
        name: branch_name.clone(),
        worktree_path: dest.clone(),
        lease_id: lease_id.clone(),
        store_root: store_root.clone(),
        repo_root: root.clone(),
        active: run_cmd.is_some(),
    };

    // Register active scratch for SIGINT/SIGTERM cleanup if a command
    // will run. Bare scratch is disarmed immediately and persists, so
    // no signal registration is needed.
    if guard.active {
        signal::register_scratch(signal::ActiveScratch {
            name: branch_name.clone(),
            worktree_path: dest.clone(),
            lease_id: lease_id.clone(),
            store_root: store_root.clone(),
            repo_root: root.clone(),
        });
    }

    match run_cmd {
        None => {
            // Bare scratch: keep the sandbox alive and disarm the guard
            guard.disarm();
            if !cfg.json {
                println!(
                    "created scratch worktree {} (branch {}, lease expires in {}s)",
                    dest.display(),
                    branch_name,
                    ttl_secs
                );
            }
            let data = ScratchData {
                worktree_path: dest.display().to_string(),
                branch: branch_name,
                lease_id,
                lease_file: lease_file_path.display().to_string(),
                expires_at,
                files_hydrated: create_data.files_hydrated,
                hydration_method: create_data.hydration_method,
                bytes_shared_cow: create_data.bytes_shared_cow,
                bytes_copied: create_data.bytes_copied,
                duration_ms: started.elapsed().as_millis() as u64,
                command: None,
                exit_code: None,
                cleaned_up: Some(false),
            };
            Ok((data, diags, None))
        }
        Some(cmd) => {
            if !cfg.json {
                eprintln!("wt-scratch: running `{cmd}` in {}", dest.display());
            }
            let mut child = Command::new("sh");
            child.arg("-c").arg(cmd).current_dir(&dest);

            if cfg.json {
                // When in --json mode, route child stdout to stderr so stdout is reserved for JSON envelope
                let stderr_fd = unsafe { libc::dup(libc::STDERR_FILENO) };
                if stderr_fd >= 0 {
                    let stdio_err = unsafe { Stdio::from_raw_fd(stderr_fd) };
                    child.stdout(stdio_err);
                }
                child.stderr(Stdio::inherit());
            } else {
                child.stdin(Stdio::inherit());
                child.stdout(Stdio::inherit());
                child.stderr(Stdio::inherit());
            }

            let status = child
                .status()
                .map_err(|e| Error::Usage(format!("failed to execute command {cmd:?}: {e}")))?;

            let exit_code = status.code().unwrap_or(1);

            // Clean up the sandbox and lease immediately
            guard.cleanup(cfg)?;

            let data = ScratchData {
                worktree_path: dest.display().to_string(),
                branch: branch_name,
                lease_id,
                lease_file: lease_file_path.display().to_string(),
                expires_at,
                files_hydrated: create_data.files_hydrated,
                hydration_method: create_data.hydration_method,
                bytes_shared_cow: create_data.bytes_shared_cow,
                bytes_copied: create_data.bytes_copied,
                duration_ms: started.elapsed().as_millis() as u64,
                command: Some(cmd.to_string()),
                exit_code: Some(exit_code),
                cleaned_up: Some(true),
            };

            Ok((data, diags, Some(exit_code)))
        }
    }
}
