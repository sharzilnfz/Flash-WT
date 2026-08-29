//! Legacy import path over the deep workspace module (market-launch
//! ticket 03): re-exports the free functions that `base`, `hydrate`,
//! `scratch`, and `demo` still consume. New code should go through
//! `crate::workspace::WorkspaceEngine` instead.

pub use crate::workspace::{default_worktree_dest, git_dir, repo_root, resolve_commit, run};
