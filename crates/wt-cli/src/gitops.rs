//! Git plumbing shared by every command (decomposed from main.rs and
//! gc.rs by arch-hardening ticket 03): locating the repository, running
//! git commands, resolving worktree git dirs, and computing the default
//! sibling destination for a new worktree.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};

/// Run git in `dir`, returning its trimmed stdout on success and its
/// trimmed stderr on failure.
pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| Error::Git(e.to_string()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(Error::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        ))
    }
}

/// The enclosing repository root of the current working directory.
pub fn repo_root() -> Result<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| Error::Git(e.to_string()))?;
    if !out.status.success() {
        return Err(Error::Git("not inside a git repository".into()));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Resolve the (absolute) git dir of a worktree. For linked worktrees
/// this lands inside the main repo's `.git/worktrees/<name>`.
pub fn git_dir(worktree: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .current_dir(worktree)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .map_err(|e| Error::Git(format!("cannot query git dir: {e}")))?;
    if !out.status.success() {
        return Err(Error::Git(
            "newly created worktree is not a git worktree".into(),
        ));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// The default destination for a new worktree: a sibling of the
/// repository named `<repo>-<name>`.
pub fn default_worktree_dest(root: &Path, name: &str) -> Result<PathBuf> {
    Ok(root
        .parent()
        .ok_or_else(|| Error::Usage("repository root has no parent".into()))?
        .join(format!(
            "{}-{name}",
            root.file_name()
                .ok_or_else(|| Error::Usage("cannot name repository directory".into()))?
                .to_string_lossy()
        )))
}
