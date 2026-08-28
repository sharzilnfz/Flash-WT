//! Lockfile parser, dependency safety classification, and lockfile
//! discovery for tiered lockfile validation (ticket 09).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::ContentId;

/// Classification of dependency safety within a project lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySafety {
    /// All dependencies are strictly pinned to immutable sources.
    Pinned,
    /// Lockfile contains local mutable references (`file:`, `link:`,
    /// `workspace:`, or unpinned git branches).
    Mutable,
}

/// Known standard lockfile names across supported language ecosystems.
const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
    "pdm.lock",
    "requirements.txt",
    "composer.lock",
    "Gemfile.lock",
];

/// Locate the nearest project lockfile associated with the heavy
/// directory `heavy_rel`, searching from its parent directory up to `repo_root`.
pub fn find_lockfile(repo_root: &Path, heavy_rel: &Path) -> Option<PathBuf> {
    let mut cur = repo_root.join(heavy_rel).parent().map(Path::to_path_buf)?;
    loop {
        for name in LOCKFILES {
            let candidate = cur.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if cur == repo_root {
            break;
        }
        match cur.parent() {
            Some(p) if p.starts_with(repo_root) || p == repo_root => cur = p.to_path_buf(),
            _ => break,
        }
    }
    None
}

/// Compute the SHA-256 content address of a lockfile.
pub fn hash_lockfile(bytes: &[u8]) -> ContentId {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    ContentId(hasher.finalize().into())
}

/// Classify whether a lockfile is strictly pinned or contains mutable
/// dependency references (`file:`, `link:`, `workspace:`, unpinned git branches).
pub fn classify_lockfile(content: &str) -> DependencySafety {
    if content.is_empty() {
        return DependencySafety::Pinned;
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        // Mutable local reference protocols
        if trimmed.contains("file:")
            || trimmed.contains("link:")
            || trimmed.contains("workspace:")
        {
            return DependencySafety::Mutable;
        }

        // Local path dependencies: e.g. path = "..." (in Cargo or Pipfile)
        if (trimmed.starts_with("path =") || trimmed.starts_with("path:"))
            && !trimmed.contains("source =")
        {
            return DependencySafety::Mutable;
        }

        // Git reference safety inspection
        if let Some(safety) = check_git_line(trimmed) {
            if safety == DependencySafety::Mutable {
                return DependencySafety::Mutable;
            }
        }
    }

    DependencySafety::Pinned
}

fn check_git_line(line: &str) -> Option<DependencySafety> {
    let is_git = line.contains("git+")
        || line.contains("git://")
        || line.contains("git@")
        || line.contains("github.com")
        || line.contains("gitlab.com")
        || line.contains("bitbucket.org")
        || line.starts_with("git =")
        || line.starts_with("source = \"git+");

    if !is_git {
        return None;
    }

    // Branch specifications without commit/rev hashes
    if (line.contains("branch =") || line.contains("branch:") || line.contains("?branch="))
        && !line.contains('#')
        && !line.contains("commit")
        && !line.contains("rev")
    {
        return Some(DependencySafety::Mutable);
    }

    // Fragment after `#`
    if let Some((_, frag)) = line.split_once('#') {
        let clean_frag = frag
            .trim_matches(|c: char| !c.is_ascii_alphanumeric())
            .split(['"', '\'', ' ', ',', ')', '}', '\r', '\n'])
            .next()
            .unwrap_or("");

        if is_hex_commit_hash(clean_frag) {
            return Some(DependencySafety::Pinned);
        } else {
            return Some(DependencySafety::Mutable);
        }
    }

    // `commit = "..."` or `rev = "..."` or `commit: "..."`
    for key in &["commit =", "commit:", "rev =", "rev:"] {
        if let Some((_, val)) = line.split_once(key) {
            let clean_val = val
                .trim_matches(|c: char| !c.is_ascii_alphanumeric())
                .split(['"', '\'', ' ', ',', ')', '}', '\r', '\n'])
                .next()
                .unwrap_or("");
            if is_hex_commit_hash(clean_val) {
                return Some(DependencySafety::Pinned);
            }
        }
    }

    // A git reference without an exact commit hash is mutable
    Some(DependencySafety::Mutable)
}

fn is_hex_commit_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_pinned_npm_lockfile() {
        let lock = r#"{
  "name": "my-app",
  "version": "1.0.0",
  "lockfileVersion": 3,
  "packages": {
    "node_modules/express": {
      "version": "4.18.2",
      "resolved": "https://registry.npmjs.org/express/-/express-4.18.2.tgz",
      "integrity": "sha512-5/PsL6iVEkiFX9XO4gkU85AZvBpBSb676Zh22wwxIDtbXXubW761W9FSkPn4fZ45h4BuQWRT5NQCmwKn3YMYg=="
    }
  }
}"#;
        assert_eq!(classify_lockfile(lock), DependencySafety::Pinned);
    }

    #[test]
    fn classifies_mutable_file_link_workspace_references() {
        let file_dep = r#"{
  "packages": {
    "node_modules/local": {
      "resolved": "file:../packages/local"
    }
  }
}"#;
        assert_eq!(classify_lockfile(file_dep), DependencySafety::Mutable);

        let link_dep = r#"packages:
  link-pkg:
    resolution: { directory: ../link-pkg }
    link: ../link-pkg
"#;
        assert_eq!(classify_lockfile(link_dep), DependencySafety::Mutable);

        let workspace_dep = r#"{
  "dependencies": {
    "core": "workspace:^1.0.0"
  }
}"#;
        assert_eq!(classify_lockfile(workspace_dep), DependencySafety::Mutable);
    }

    #[test]
    fn classifies_git_references_pinned_vs_unpinned() {
        let unpinned_branch = r#"{
  "packages": {
    "node_modules/foo": {
      "resolved": "git+https://github.com/foo/bar.git#main"
    }
  }
}"#;
        assert_eq!(classify_lockfile(unpinned_branch), DependencySafety::Mutable);

        let pinned_sha = r#"{
  "packages": {
    "node_modules/foo": {
      "resolved": "git+https://github.com/foo/bar.git#a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
    }
  }
}"#;
        assert_eq!(classify_lockfile(pinned_sha), DependencySafety::Pinned);

        let cargo_unpinned = r#"
[[package]]
name = "tokio"
source = "git+https://github.com/tokio-rs/tokio?branch=master"
"#;
        assert_eq!(classify_lockfile(cargo_unpinned), DependencySafety::Mutable);

        let cargo_pinned = r#"
[[package]]
name = "tokio"
source = "git+https://github.com/tokio-rs/tokio?branch=master#0123456789abcdef0123456789abcdef01234567"
"#;
        assert_eq!(classify_lockfile(cargo_pinned), DependencySafety::Pinned);
    }

    #[test]
    fn finds_lockfile_in_parent_or_ancestors() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path();
        let packages = root.join("packages/app");
        std::fs::create_dir_all(&packages).unwrap();
        let lockfile = root.join("package-lock.json");
        std::fs::write(&lockfile, "{}").unwrap();

        let found = find_lockfile(root, Path::new("packages/app/node_modules")).unwrap();
        assert_eq!(found, lockfile);

        let nested_lock = packages.join("package-lock.json");
        std::fs::write(&nested_lock, "{}").unwrap();
        let found_nested = find_lockfile(root, Path::new("packages/app/node_modules")).unwrap();
        assert_eq!(found_nested, nested_lock);
    }
}
