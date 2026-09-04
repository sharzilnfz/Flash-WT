use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::ContentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySafety {
    Pinned,

    Mutable,
}

pub const LOCKFILES: &[&str] = &[
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

pub fn package_manager_command(lockfile_name: &str) -> &'static str {
    match lockfile_name {
        "package-lock.json" | "npm-shrinkwrap.json" => "npm install",
        "pnpm-lock.yaml" => "pnpm install",
        "yarn.lock" => "yarn install",
        "bun.lockb" | "bun.lock" => "bun install",
        "Cargo.lock" => "cargo build",
        "poetry.lock" => "poetry install",
        "Pipfile.lock" => "pipenv install",
        "pdm.lock" => "pdm install",
        "requirements.txt" => "pip install -r requirements.txt",
        "composer.lock" => "composer install",
        "Gemfile.lock" => "bundle install",
        _ => "your package manager",
    }
}

pub fn find_lockfile_rel(repo_root: &Path, heavy_rel: &Path) -> Option<PathBuf> {
    find_lockfile(repo_root, heavy_rel).and_then(|abs| abs.strip_prefix(repo_root).ok().map(|p| p.to_path_buf()))
}

pub fn find_lockfile(repo_root: &Path, heavy_rel: &Path) -> Option<PathBuf> {
    let full_path = repo_root.join(heavy_rel);
    let start = full_path.parent()?;
    for dir in start.ancestors() {
        if !dir.starts_with(repo_root) {
            break;
        }
        for name in LOCKFILES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if dir == repo_root {
            break;
        }
    }
    None
}

pub fn hash_lockfile(bytes: &[u8]) -> ContentId {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    ContentId(hasher.finalize().into())
}

pub fn classify_lockfile(content: &str) -> DependencySafety {
    if content.is_empty() {
        return DependencySafety::Pinned;
    }

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }

        if trimmed.contains("file:") || trimmed.contains("link:") || trimmed.contains("workspace:")
        {
            return DependencySafety::Mutable;
        }

        if (trimmed.starts_with("path =") || trimmed.starts_with("path:"))
            && !trimmed.contains("source =")
        {
            return DependencySafety::Mutable;
        }

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

    if (line.contains("branch =") || line.contains("branch:") || line.contains("?branch="))
        && !line.contains('#')
        && !line.contains("commit")
        && !line.contains("rev")
    {
        return Some(DependencySafety::Mutable);
    }

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

    Some(DependencySafety::Mutable)
}

fn is_hex_commit_hash(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
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
        assert_eq!(
            classify_lockfile(unpinned_branch),
            DependencySafety::Mutable
        );

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

        let rel = find_lockfile_rel(root, Path::new("packages/app/node_modules")).unwrap();
        assert_eq!(rel, PathBuf::from("packages/app/package-lock.json"));
    }

    #[test]
    fn maps_lockfile_names_to_package_managers() {
        assert_eq!(package_manager_command("package-lock.json"), "npm install");
        assert_eq!(package_manager_command("pnpm-lock.yaml"), "pnpm install");
        assert_eq!(package_manager_command("yarn.lock"), "yarn install");
        assert_eq!(package_manager_command("bun.lock"), "bun install");
        assert_eq!(package_manager_command("Cargo.lock"), "cargo build");
    }
}
