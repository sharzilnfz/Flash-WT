use crate::envelope::Diagnostic;
use crate::workspace;
use flashwt_store::DiskStore;
use std::path::Path;

pub fn check_base_movement(
    store: &DiskStore,
    repo_root: &Path,
    base_ref: Option<&str>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    if let Ok(git_dir) = workspace::git_dir(repo_root) {
        if let Ok(Some(m)) = store.read_worktree_mirror(repo_root, &git_dir) {
            if let (Some(base_branch), Some(base_commit)) = (&m.base_branch, &m.base_commit) {
                if let Ok(current_commit) = workspace::resolve_commit(repo_root, base_branch) {
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

    if let Some(target_base) = base_ref {
        let repo_canonical = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.to_path_buf());
        for read in flashwt_store::read_mirrors(store.root()) {
            if let Ok(m) = read.mirror {
                let mirror_repo = match workspace::repo_root_from_gitdir(&m.gitdir) {
                    Some(p) => p,
                    None => continue,
                };
                let mirror_canonical = mirror_repo
                    .canonicalize()
                    .unwrap_or_else(|_| mirror_repo.clone());
                if mirror_canonical != repo_canonical && mirror_repo != repo_root {
                    continue;
                }

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
                        if let Ok(current_commit) =
                            workspace::resolve_commit(repo_root, parent_base)
                        {
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

pub fn check_worktree_base_movement(
    store: &DiskStore,
    repo_root: &Path,
    worktree: &Path,
    git_dir: &Path,
) -> Option<Diagnostic> {
    if let Ok(Some(m)) = store.read_worktree_mirror(worktree, git_dir) {
        if let (Some(base_branch), Some(base_commit)) = (&m.base_branch, &m.base_commit) {
            if let Ok(current_commit) = workspace::resolve_commit(repo_root, base_branch) {
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
        let git_dir = workspace::git_dir(&worktree).unwrap();

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

        assert!(check_worktree_base_movement(&store, &repo, &worktree, &git_dir).is_none());

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

        let resolved = workspace::resolve_commit(&repo, "main").unwrap();
        assert_eq!(resolved, c2);

        let diag = check_worktree_base_movement(&store, &repo, &worktree, &git_dir)
            .expect("diagnostic expected");
        assert_eq!(diag.code, "BASE_BRANCH_MOVED");
        assert!(diag.message.contains("main"));
        assert!(diag.message.contains(&c1));
        assert!(diag.message.contains(&c2));
    }

    #[test]
    fn check_base_movement_ignores_other_repo_mirrors() {
        let temp = tempfile::tempdir().unwrap();
        let store_dir = temp.path().join("store");
        let repo_a = temp.path().join("repo-a");
        let repo_b = temp.path().join("repo-b");
        for repo in [&repo_a, &repo_b] {
            fs::create_dir_all(repo).unwrap();
            git(repo, &["init"]);
            git(repo, &["config", "user.name", "Test"]);
            git(repo, &["config", "user.email", "test@example.com"]);
            fs::write(repo.join("file.txt"), "hello").unwrap();
            git(repo, &["add", "file.txt"]);
            git(repo, &["commit", "-m", "initial"]);
            git(repo, &["branch", "-M", "main"]);
        }
        let c1_a = git(&repo_a, &["rev-parse", "HEAD"]);
        let c1_b = git(&repo_b, &["rev-parse", "HEAD"]);

        let worktree_a = temp.path().join("repo-a-feat");
        git(
            &repo_a,
            &[
                "worktree",
                "add",
                "-b",
                "feat",
                &worktree_a.to_string_lossy(),
                "main",
            ],
        );
        let git_dir_a = workspace::git_dir(&worktree_a).unwrap();

        let worktree_b = temp.path().join("repo-b-feat");
        git(
            &repo_b,
            &[
                "worktree",
                "add",
                "-b",
                "feat",
                &worktree_b.to_string_lossy(),
                "main",
            ],
        );
        let git_dir_b = workspace::git_dir(&worktree_b).unwrap();

        let store = DiskStore::open(&store_dir).unwrap();
        store
            .publish_worktree_mirror(
                &worktree_a,
                &git_dir_a,
                std::iter::empty(),
                std::iter::empty(),
                Some("main"),
                Some(&c1_a),
            )
            .unwrap();
        store
            .publish_worktree_mirror(
                &worktree_b,
                &git_dir_b,
                std::iter::empty(),
                std::iter::empty(),
                Some("main"),
                Some(&c1_b),
            )
            .unwrap();

        fs::write(repo_a.join("file2.txt"), "world").unwrap();
        git(&repo_a, &["add", "file2.txt"]);
        git(&repo_a, &["commit", "-m", "second"]);
        let c2_a = git(&repo_a, &["rev-parse", "HEAD"]);
        assert_ne!(c1_a, c2_a);

        assert_eq!(c1_b, git(&repo_b, &["rev-parse", "HEAD"]));

        let diags_b = check_base_movement(&store, &repo_b, Some("feat"));
        assert!(
            diags_b.is_empty(),
            "repo-b should ignore repo-a's moved mirror, got {diags_b:?}"
        );

        let diags_a = check_base_movement(&store, &repo_a, Some("feat"));
        assert!(
            !diags_a.is_empty(),
            "repo-a should report its own moved parent"
        );
        assert!(diags_a.iter().any(|d| d.code == "BASE_BRANCH_MOVED"));
    }
}
