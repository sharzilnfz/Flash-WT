//! Base branch tracking and movement diagnostics (ticket 02).

use crate::envelope::Diagnostic;
use crate::gitops;
use std::path::Path;
use wt_store::DiskStore;

/// Check whether the base branch of the current worktree (or a referenced base) has moved
/// since worktree initialization.
pub fn check_base_movement(
    store: &DiskStore,
    repo_root: &Path,
    base_ref: Option<&str>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    // 1. Check the current worktree itself (if we are in a linked worktree with a mirror)
    if let Ok(git_dir) = gitops::git_dir(repo_root) {
        if let Ok(Some(m)) = store.read_worktree_mirror(repo_root, &git_dir) {
            if let (Some(base_branch), Some(base_commit)) = (&m.base_branch, &m.base_commit) {
                if let Ok(current_commit) = gitops::resolve_commit(repo_root, base_branch) {
                    if current_commit != *base_commit {
                        diagnostics.push(Diagnostic::warning(
                            "BASE_BRANCH_MOVED",
                            format!(
                                "Base branch '{base_branch}' has moved from {base_commit} to {current_commit}"
                            ),
                        ));
                    }
                }
            }
        }
    }

    // 2. If a base_ref was passed (e.g. `wt create <name> --base <base_ref>`),
    // check if base_ref itself is an existing worktree whose base has moved!
    if let Some(target_base) = base_ref {
        for read in wt_store::read_mirrors(store.root()) {
            if let Ok(m) = read.mirror {
                let is_matching_worktree = m
                    .worktree
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| {
                        name == target_base || name.ends_with(&format!("-{target_base}"))
                    });
                let is_matching_gitdir = m
                    .gitdir
                    .to_string_lossy()
                    .ends_with(&format!("/worktrees/{target_base}"));

                if is_matching_worktree || is_matching_gitdir {
                    if let (Some(parent_base), Some(parent_commit)) =
                        (&m.base_branch, &m.base_commit)
                    {
                        if let Ok(current_commit) = gitops::resolve_commit(repo_root, parent_base) {
                            if current_commit != *parent_commit {
                                let diag = Diagnostic::warning(
                                    "BASE_BRANCH_MOVED",
                                    format!(
                                        "Parent base branch '{parent_base}' of '{target_base}' has moved from {parent_commit} to {current_commit}"
                                    ),
                                );
                                if !diagnostics.contains(&diag) {
                                    diagnostics.push(diag);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    diagnostics
}

/// Check base movement for a specific worktree being operated on (e.g. before removal).
pub fn check_worktree_base_movement(
    store: &DiskStore,
    repo_root: &Path,
    worktree: &Path,
    git_dir: &Path,
) -> Option<Diagnostic> {
    if let Ok(Some(m)) = store.read_worktree_mirror(worktree, git_dir) {
        if let (Some(base_branch), Some(base_commit)) = (&m.base_branch, &m.base_commit) {
            if let Ok(current_commit) = gitops::resolve_commit(repo_root, base_branch) {
                if current_commit != *base_commit {
                    return Some(Diagnostic::warning(
                        "BASE_BRANCH_MOVED",
                        format!(
                            "Base branch '{base_branch}' has moved from {base_commit} to {current_commit}"
                        ),
                    ));
                }
            }
        }
    }
    None
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn check_worktree_base_movement_detects_changes() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let store_dir = temp.path().join("store");
        fs::create_dir_all(&repo).unwrap();

        git(&repo, &["init"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        fs::write(repo.join("file.txt"), "hello").unwrap();
        git(&repo, &["add", "file.txt"]);
        git(&repo, &["commit", "-m", "initial"]);
        git(&repo, &["branch", "-M", "main"]);

        let c1 = git(&repo, &["rev-parse", "HEAD"]);

        let worktree = temp.path().join("repo-feat");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feat",
                &worktree.to_string_lossy(),
                "main",
            ],
        );
        let git_dir = gitops::git_dir(&worktree).unwrap();

        let store = DiskStore::open(&store_dir).unwrap();
        store
            .publish_worktree_mirror(
                &worktree,
                &git_dir,
                std::iter::empty(),
                std::iter::empty(),
                Some("main"),
                Some(&c1),
            )
            .unwrap();

        // Initially no movement
        assert!(check_worktree_base_movement(&store, &repo, &worktree, &git_dir).is_none());

        // Advance main
        fs::write(repo.join("file2.txt"), "world").unwrap();
        git(&repo, &["add", "file2.txt"]);
        git(&repo, &["commit", "-m", "second"]);
        let c2 = git(&repo, &["rev-parse", "HEAD"]);
        assert_ne!(c1, c2);

        let mirror = store
            .read_worktree_mirror(&worktree, &git_dir)
            .unwrap()
            .expect("mirror exists");
        assert_eq!(mirror.base_branch.as_deref(), Some("main"));
        assert_eq!(mirror.base_commit.as_deref(), Some(c1.as_str()));

        let resolved = gitops::resolve_commit(&repo, "main").unwrap();
        assert_eq!(resolved, c2);

        // Now movement is detected
        let diag = check_worktree_base_movement(&store, &repo, &worktree, &git_dir)
            .expect("diagnostic expected");
        assert_eq!(diag.code, "BASE_BRANCH_MOVED");
        assert!(diag.message.contains("main"));
        assert!(diag.message.contains(&c1));
        assert!(diag.message.contains(&c2));
    }
}
