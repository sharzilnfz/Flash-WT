//! Deep workspace module (market-launch ticket 03): the single seam for
//! everything about git worktrees. Repository root detection, git
//! command execution, gitdir resolution, porcelain worktree parsing,
//! metadata mapping, active checkout detection, default destination
//! derivation, merge-ancestor validation, and worktree/branch removal
//! all live here behind [`WorkspaceEngine`]. Command handlers for
//! listing, cleanup, creation, and scratch isolation interact only
//! with this interface.

use std::fs;
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
    let cwd = std::env::current_dir().map_err(|e| Error::Git(e.to_string()))?;
    run(&cwd, &["rev-parse", "--show-toplevel"])
        .map_err(|_| Error::Git("not inside a git repository".into()))
        .map(PathBuf::from)
}

/// Resolve the (absolute) git dir of a worktree. For linked worktrees
/// this lands inside the main repo's `.git/worktrees/<name>`.
pub fn git_dir(worktree: &Path) -> Result<PathBuf> {
    run(worktree, &["rev-parse", "--absolute-git-dir"])
        .map_err(|_| Error::Git("newly created worktree is not a git worktree".into()))
        .map(PathBuf::from)
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

/// Resolve a git reference or branch name to its full commit SHA.
pub fn resolve_commit(dir: &Path, rev: &str) -> Result<String> {
    let peel = format!("{rev}^{{commit}}");
    run(dir, &["rev-parse", "--verify", &peel])
        .or_else(|_| run(dir, &["rev-parse", "--verify", rev]))
}

/// Recover the repository root from a worktree's git dir, following
/// the `commondir` pointer that linked worktrees write.
pub fn repo_root_from_gitdir(gitdir: &Path) -> Option<PathBuf> {
    if gitdir.exists() {
        let commondir = gitdir.join("commondir");
        if let Ok(rel) = fs::read_to_string(&commondir) {
            let mgd = gitdir.join(rel.trim());
            if let Ok(canon) = mgd.canonicalize() {
                if canon.file_name() == Some(std::ffi::OsStr::new(".git")) {
                    if let Some(parent) = canon.parent() {
                        return Some(parent.to_path_buf());
                    }
                }
                return Some(canon);
            }
        }
    }

    if let Some(parent) = gitdir.parent() {
        if parent.file_name() == Some(std::ffi::OsStr::new("worktrees")) {
            if let Some(git_parent) = parent.parent() {
                if git_parent.file_name() == Some(std::ffi::OsStr::new(".git")) {
                    if let Some(repo) = git_parent.parent() {
                        if repo.is_dir() {
                            return Some(repo.to_path_buf());
                        }
                    }
                }
            }
        }
    }

    None
}

/// Resolve the git directory of a worktree from the filesystem when
/// possible (`.git` dir for the main worktree, `gitdir:` pointer file
/// for linked ones), falling back to asking git itself.
pub fn resolve_git_dir(worktree_path: &Path) -> PathBuf {
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
    if let Ok(dir_str) = run(worktree_path, &["rev-parse", "--absolute-git-dir"]) {
        return PathBuf::from(dir_str);
    }
    dot_git
}

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
            let branch_name = branch_ref.strip_prefix("refs/heads/").unwrap_or(branch_ref);
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

/// A porcelain record mapped to the metadata consumers need: the raw
/// fields plus the resolved git dir, the display branch, and the
/// main-worktree verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeMetadata {
    pub raw: RawGitWorktree,
    pub git_dir: PathBuf,
    pub branch_display: String,
    pub is_main: bool,
}

/// Typed engine over one git repository's worktrees. Construct with
/// [`WorkspaceEngine::discover`] (root from the current working
/// directory) or [`WorkspaceEngine::from_root`].
#[derive(Debug, Clone)]
pub struct WorkspaceEngine {
    root: PathBuf,
}

impl WorkspaceEngine {
    /// Discover the enclosing repository of the current working directory.
    pub fn discover() -> Result<Self> {
        Ok(Self { root: repo_root()? })
    }

    /// Anchor the engine at an explicit repository root.
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// The repository root this engine operates on.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Run git in the repository root.
    pub fn git(&self, args: &[&str]) -> Result<String> {
        run(&self.root, args)
    }

    /// Enumerate all worktrees via porcelain output.
    pub fn worktrees(&self) -> Result<Vec<RawGitWorktree>> {
        let porcelain = self.git(&["worktree", "list", "--porcelain"])?;
        Ok(parse_git_worktrees(&porcelain))
    }

    /// Map a raw porcelain record to full metadata: resolved git dir,
    /// display branch, and main-worktree verdict.
    pub fn metadata(&self, raw: RawGitWorktree) -> WorktreeMetadata {
        let worktree_path = raw.path.clone();
        WorktreeMetadata {
            is_main: self.is_main(&worktree_path),
            branch_display: branch_display(&raw),
            git_dir: resolve_git_dir(&worktree_path),
            raw,
        }
    }

    /// Enumerate all worktrees with metadata mapped.
    pub fn worktree_metadata(&self) -> Result<Vec<WorktreeMetadata>> {
        Ok(self
            .worktrees()?
            .into_iter()
            .map(|raw| self.metadata(raw))
            .collect())
    }

    /// Whether `branch` is fully merged into the current HEAD.
    /// Unparseable branch names and git failures count as unmerged.
    pub fn is_branch_merged(&self, branch: &str) -> bool {
        !branch.is_empty()
            && self
                .git(&["merge-base", "--is-ancestor", branch, "HEAD"])
                .is_ok()
    }

    /// Whether the current working directory sits inside `worktree`.
    pub fn is_active(&self, worktree: &Path) -> bool {
        let Some(cwd) = std::env::current_dir()
            .ok()
            .and_then(|d| d.canonicalize().ok())
        else {
            return false;
        };
        let canon = worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_path_buf());
        cwd == canon || cwd.starts_with(&canon)
    }

    /// Whether `worktree` is the main worktree: it either carries a
    /// real `.git` directory or is the repository root itself.
    pub fn is_main(&self, worktree: &Path) -> bool {
        if worktree.join(".git").is_dir() {
            return true;
        }
        let canon = worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_path_buf());
        let canon_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        canon == canon_root
    }

    /// The default destination for a new worktree: a sibling of the
    /// repository named `<repo>-<name>`.
    pub fn default_dest(&self, name: &str) -> Result<PathBuf> {
        default_worktree_dest(&self.root, name)
    }

    /// Resolve a git reference or branch name to its full commit SHA.
    pub fn resolve_commit(&self, rev: &str) -> Result<String> {
        resolve_commit(&self.root, rev)
    }

    /// Create a worktree at `dest` for `name`, branching from
    /// `start_point`. An existing branch falls back to checking it out
    /// directly.
    pub fn create_worktree(&self, name: &str, dest: &Path, start_point: &str) -> Result<()> {
        let dest_text = dest.to_string_lossy().into_owned();
        self.git(&["worktree", "add", "-b", name, &dest_text, start_point])
            .or_else(|_| self.git(&["worktree", "add", &dest_text, name]))?;
        Ok(())
    }

    /// Remove a worktree, with a forced retry when the plain removal
    /// refuses. Best-effort: errors are swallowed so cleanup flows can
    /// continue past half-removed directories.
    pub fn remove_worktree_lenient(&self, dest: &Path) {
        let dest_text = dest.to_string_lossy().into_owned();
        let _ = self
            .git(&["worktree", "remove", "--force", &dest_text])
            .or_else(|_| self.git(&["worktree", "remove", &dest_text]));
    }

    /// Remove a worktree, surfacing git's refusal as an error.
    pub fn remove_worktree(&self, dest: &Path) -> Result<()> {
        self.git(&["worktree", "remove", &dest.to_string_lossy()])
            .map(|_| ())
    }

    /// Delete a branch regardless of its merge state.
    pub fn delete_branch(&self, name: &str) -> Result<String> {
        self.git(&["branch", "-D", name])
    }
}

fn branch_display(raw: &RawGitWorktree) -> String {
    if let Some(ref b) = raw.branch {
        b.clone()
    } else if raw.is_detached {
        "(detached)".to_string()
    } else if raw.is_bare {
        "(bare)".to_string()
    } else {
        "(unknown)".to_string()
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
    fn branch_display_mapping() {
        let raw = RawGitWorktree {
            path: PathBuf::from("/p"),
            head: None,
            branch: Some("feature".into()),
            is_detached: false,
            is_bare: false,
            is_locked: false,
            is_prunable: false,
        };
        assert_eq!(branch_display(&raw), "feature");

        let detached = RawGitWorktree {
            is_detached: true,
            branch: None,
            ..raw.clone()
        };
        assert_eq!(branch_display(&detached), "(detached)");

        let bare = RawGitWorktree {
            is_bare: true,
            branch: None,
            ..raw.clone()
        };
        assert_eq!(branch_display(&bare), "(bare)");

        let unknown = RawGitWorktree {
            branch: None,
            ..raw
        };
        assert_eq!(branch_display(&unknown), "(unknown)");
    }
}
