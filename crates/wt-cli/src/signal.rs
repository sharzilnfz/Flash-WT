//! Signal handling for transactional cleanup (ticket 05).
//!
//! Registers `SIGINT`/`SIGTERM` handlers that clean up active scratch
//! workspaces and in-flight `wt new` worktrees before exiting. The
//! handlers run on a dedicated thread via `signal-hook::iterator::Signals`
//! so normal IO and locking are safe. Active guards are tracked via
//! global mutexes populated by `commands/create` and `commands/scratch`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::thread;

#[derive(Debug, Clone)]
pub struct ActiveScratch {
    pub name: String,
    pub worktree_path: PathBuf,
    pub lease_id: String,
    pub store_root: PathBuf,
    pub repo_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ActiveCreate {
    pub name: String,
    pub dest: PathBuf,
    pub repo_root: PathBuf,
}

static ACTIVE_SCRATCH: OnceLock<Mutex<Option<ActiveScratch>>> = OnceLock::new();
static ACTIVE_CREATE: OnceLock<Mutex<Option<ActiveCreate>>> = OnceLock::new();

fn scratch_lock() -> &'static Mutex<Option<ActiveScratch>> {
    ACTIVE_SCRATCH.get_or_init(|| Mutex::new(None))
}

fn create_lock() -> &'static Mutex<Option<ActiveCreate>> {
    ACTIVE_CREATE.get_or_init(|| Mutex::new(None))
}

/// Register the currently active scratch worktree for signal cleanup.
pub fn register_scratch(active: ActiveScratch) {
    if let Ok(mut g) = scratch_lock().lock() {
        *g = Some(active);
    }
}

/// Clear the active scratch registration (called after successful cleanup or disarm).
pub fn clear_scratch() {
    if let Ok(mut g) = scratch_lock().lock() {
        *g = None;
    }
}

/// Register an in-flight `wt new` worktree for signal cleanup.
pub fn register_create(active: ActiveCreate) {
    if let Ok(mut g) = create_lock().lock() {
        *g = Some(active);
    }
}

/// Clear the active create registration.
pub fn clear_create() {
    if let Ok(mut g) = create_lock().lock() {
        *g = None;
    }
}

/// Best-effort removal of a newly created worktree+branch.
/// Used both by the signal handler and by the transactional rollback
/// in `commands/create`.
pub fn rollback_create(name: &str, dest: &Path, repo_root: &Path) {
    let dest_str = dest.to_string_lossy().into_owned();
    // `git worktree remove --force` then prune + branch delete. All
    // best-effort — failures are swallowed so the caller can still
    // return the original hydration error.
    let _ = Command::new("git")
        .current_dir(repo_root)
        .args(["worktree", "remove", "--force", &dest_str])
        .output();
    let _ = Command::new("git")
        .current_dir(repo_root)
        .args(["worktree", "prune"])
        .output();
    let _ = Command::new("git")
        .current_dir(repo_root)
        .args(["branch", "-D", name])
        .output();
    if dest.exists() {
        let _ = fs::remove_dir_all(dest);
    }
}

/// Initialize background thread that waits for `SIGINT`/`SIGTERM` and
/// performs cleanup before exiting with `128 + signal`.
pub fn init_signal_handlers() {
    // On non-unix platforms signal-hook still compiles but Signals for
    // SIGTERM may not be meaningful. The thread is still spawned.
    let signals_result = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ]);

    let Ok(mut signals) = signals_result else {
        return;
    };

    thread::spawn(move || {
        for sig in signals.forever() {
            eprintln!("wt: received signal {sig}, cleaning up...");
            // Drain active guards. Use try_lock to avoid deadlock if
            // the signal arrived while the main thread holds the lock.
            cleanup_scratch();
            cleanup_create();
            // Exit with conventional 128+signal status.
            std::process::exit(128 + sig);
        }
    });
}

fn cleanup_scratch() {
    let active = {
        let Ok(mut g) = scratch_lock().try_lock() else {
            return;
        };
        g.take()
    };
    let Some(a) = active else {
        return;
    };

    // Remove lease file.
    let _ = wt_store::remove_lease(&a.store_root, &a.lease_id);

    // Remove worktree via git if it exists.
    let dest_str = a.worktree_path.to_string_lossy().into_owned();
    let _ = Command::new("git")
        .current_dir(&a.repo_root)
        .args(["worktree", "remove", "--force", &dest_str])
        .output();
    let _ = Command::new("git")
        .current_dir(&a.repo_root)
        .args(["worktree", "prune"])
        .output();
    let _ = Command::new("git")
        .current_dir(&a.repo_root)
        .args(["branch", "-D", &a.name])
        .output();
    if a.worktree_path.exists() {
        let _ = fs::remove_dir_all(&a.worktree_path);
    }
    // Also remove gitdir under .git/worktrees/<name> if still present.
    let git_worktree_dir = a.repo_root.join(".git").join("worktrees").join(&a.name);
    if git_worktree_dir.exists() {
        let _ = fs::remove_dir_all(&git_worktree_dir);
    }
}

fn cleanup_create() {
    let active = {
        let Ok(mut g) = create_lock().try_lock() else {
            return;
        };
        g.take()
    };
    let Some(a) = active else {
        return;
    };
    rollback_create(&a.name, &a.dest, &a.repo_root);
}
