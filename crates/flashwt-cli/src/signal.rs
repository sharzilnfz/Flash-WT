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

pub fn register_scratch(active: ActiveScratch) {
    if let Ok(mut g) = scratch_lock().lock() {
        *g = Some(active);
    }
}

pub fn clear_scratch() {
    if let Ok(mut g) = scratch_lock().lock() {
        *g = None;
    }
}

pub fn register_create(active: ActiveCreate) {
    if let Ok(mut g) = create_lock().lock() {
        *g = Some(active);
    }
}

pub fn clear_create() {
    if let Ok(mut g) = create_lock().lock() {
        *g = None;
    }
}

pub fn rollback_create(name: &str, dest: &Path, repo_root: &Path) {
    let dest_str = dest.to_string_lossy().into_owned();

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

pub fn init_signal_handlers() {
    let signals_result = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ]);

    let Ok(mut signals) = signals_result else {
        return;
    };

    thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            eprintln!("flashwt: received signal {sig}, cleaning up...");

            cleanup_scratch();
            cleanup_create();

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

    let _ = flashwt_store::remove_lease(&a.store_root, &a.lease_id);

    rollback_create(&a.name, &a.worktree_path, &a.repo_root);

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
