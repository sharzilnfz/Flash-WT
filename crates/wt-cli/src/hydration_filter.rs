//! The `.wtinclude` hydration filter: parsing, gitignore-style pattern
//! matching against repo-relative directory paths, directory discovery,
//! starter-manifest creation, and volatile compiler cache exclusions.
//!
//! Everything here is pure filesystem-and-strings logic, unit-tested in
//! place — no binary spawn required.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Used when no manifest exists yet. Deliberately short and boring:
/// these cover the ecosystems that actually produce untracked bulk,
/// while excluding volatile compiler caches to prevent cache corruption.
pub const DEFAULT_PATTERNS: &[&str] = &[
    "node_modules/",
    "!node_modules/.vite/",
    "target/",
    "!target/debug/incremental/",
    "dist/",
    "build/",
    ".cache/",
    ".venv/",
    "__pycache__/",
    "!.next/cache/",
];

/// Default starter manifest template written to `.wtinclude`.
pub const STARTER_MANIFEST: &str = "\
# wt: directories hydrated into every new worktree.
# Gitignore syntax, relative to this repository root. Edit freely;
# anything listed here is copied (never moved) from this checkout.
node_modules/
!node_modules/.vite/
target/
!target/debug/incremental/
dist/
build/
.cache/
.venv/
__pycache__/
!.next/cache/
";

/// Determines whether a repo-relative path matches a volatile host compiler cache
/// that should be excluded from starter manifests and hydration.
///
/// Volatile caches include:
/// - Rust incremental compiler caches (`target/debug/incremental/`, `target/**/incremental/`)
/// - Next.js build cache (`.next/cache/`)
/// - Vite dependency cache (`node_modules/.vite/`)
pub fn is_volatile_cache(rel_path: &str) -> bool {
    let normalized = rel_path.trim_start_matches('/').trim_end_matches('/');
    let segs: Vec<&str> = normalized.split('/').collect();

    for (i, &seg) in segs.iter().enumerate() {
        if seg == "target" {
            if segs.get(i + 1) == Some(&"incremental") {
                return true;
            }
            if segs.get(i + 2) == Some(&"incremental") {
                return true;
            }
        }
        if seg == "incremental" && i > 0 && segs[..i].contains(&"target") {
            return true;
        }
        if seg == ".next" && segs.get(i + 1) == Some(&"cache") {
            return true;
        }
        if seg == "cache" && i > 0 && segs[..i].ends_with(&[".next"]) {
            return true;
        }
        if seg == "node_modules" && segs.get(i + 1) == Some(&".vite") {
            return true;
        }
        if seg == ".vite" && i > 0 && segs[..i].ends_with(&["node_modules"]) {
            return true;
        }
    }

    false
}

/// What [`load_patterns`] decided. Splitting the decision from the
/// printing keeps this module side-effect-free above the filesystem:
/// the caller owns user-visible output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadedPatterns {
    /// An existing manifest was read.
    Loaded {
        /// Path to the loaded manifest.
        path: PathBuf,
        /// Patterns parsed from existing manifest.
        patterns: Vec<String>,
    },
    /// No manifest file existed; in-memory defaults are used without writing to disk.
    Defaults {
        /// Default patterns.
        patterns: Vec<String>,
    },
}

impl LoadedPatterns {
    pub fn is_defaults(&self) -> bool {
        matches!(self, LoadedPatterns::Defaults { .. })
    }

    pub fn patterns(&self) -> &[String] {
        match self {
            LoadedPatterns::Loaded { patterns, .. } => patterns,
            LoadedPatterns::Defaults { patterns } => patterns,
        }
    }
}

/// Explanation for why hydration yielded zero savings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroSavingsReason {
    /// No configured or default heavy directory patterns matched in the source repo.
    NoMatchingDirectories,
    /// Matching directories were found, but they contained no files to hydrate.
    NoFilesHydrated,
}

impl ZeroSavingsReason {
    /// Machine-readable diagnostic code for envelopes.
    pub const DIAGNOSTIC_CODE: &'static str = "ZERO_SAVINGS";

    /// Human-friendly plain-language explanation.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::NoMatchingDirectories => {
                "no matching heavy directories found and the worktree relies strictly on git tracking"
            }
            Self::NoFilesHydrated => {
                "no files hydrated from matching directories and the worktree relies strictly on git tracking"
            }
        }
    }

    /// Notice printed when hydration detects nothing to hydrate.
    pub fn human_notice(self) -> String {
        format!("nothing to hydrate ({})", self.explanation())
    }

    /// Summary line printed during create or hydrate completion.
    pub fn human_summary(self) -> String {
        format!("0 bytes saved ({})", self.explanation())
    }
}

/// Parse manifest text into patterns, skipping blank lines and
/// `#` comments. Preserves negation (`!`) patterns for exclusions.
pub fn parse_patterns(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Gitignore-style match of one pattern against a repo-relative
/// directory path. Patterns without an interior `/` match a single
/// path segment at any depth; anchored patterns must match from the
/// root. `*` wildcards within a segment and `**` across segments.
pub fn pattern_matches(pattern: &str, rel: &Path) -> bool {
    let pat = pattern.trim_end_matches('/').trim_start_matches('/');
    if pat.is_empty() {
        return false;
    }
    let segs: Vec<&str> = pat.split('/').collect();
    let rel_text = rel.to_string_lossy();
    let rel_clean = rel_text.trim_start_matches('/').trim_end_matches('/');
    if rel_clean.is_empty() {
        return false;
    }
    let path_segs: Vec<&str> = rel_clean.split('/').collect();
    if pat.contains('/') {
        glob_match(&segs, &path_segs)
    } else {
        path_segs.iter().any(|seg| segment_match(pat, seg))
    }
}

fn glob_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| glob_match(rest, &path[i..])),
        Some((p, rest)) => match path.split_first() {
            Some((seg, tail)) if segment_match(p, seg) => glob_match(rest, tail),
            _ => false,
        },
    }
}

fn segment_match(pattern: &str, segment: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == segment,
        Some((prefix, suffix)) => {
            segment.len() >= prefix.len() + suffix.len()
                && segment.starts_with(prefix)
                && segment.ends_with(suffix)
        }
    }
}

/// Every existing directory under `root` (`.git` pruned) matching at
/// least one include pattern, sorted, with matches nested inside an
/// earlier match dropped (the outer copy already covers them).
pub fn collect_matches(root: &Path, patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut matched = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let positive_patterns: Vec<&str> = patterns
        .iter()
        .map(String::as_str)
        .filter(|p| !p.starts_with('!'))
        .collect();
    let negative_patterns: Vec<&str> = patterns
        .iter()
        .map(String::as_str)
        .filter(|p| p.starts_with('!'))
        .map(|p| p.trim_start_matches('!'))
        .collect();

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || entry.file_name() == ".git" {
                continue;
            }
            let rel = path.strip_prefix(root).map_err(|_| {
                Error::Store(format!(
                    "pattern matched path outside repository root: {}",
                    path.display()
                ))
            })?;
            let rel_str = rel.to_string_lossy();
            if negative_patterns.iter().any(|p| pattern_matches(p, rel))
                || is_volatile_cache(&rel_str)
            {
                continue;
            }
            if positive_patterns.iter().any(|p| pattern_matches(p, rel)) {
                matched.push(rel.to_path_buf());
            } else {
                stack.push(path);
            }
        }
    }
    matched.sort();
    Ok(matched
        .into_iter()
        .scan(None::<PathBuf>, |prev, rel| {
            let covered = prev.as_ref().is_some_and(|p| rel.starts_with(p));
            if !covered {
                *prev = Some(rel.clone());
            }
            Some((!covered).then_some(rel))
        })
        .flatten()
        .collect())
}

/// Decide which patterns hydrate from: the manifest at `manifest`
/// when given, else `<root>/.wtinclude`. A missing default-path
/// manifest is not an error — defaults apply in-memory without
/// auto-writing to disk.
pub fn load_patterns(root: &Path, manifest: Option<&Path>) -> Result<LoadedPatterns> {
    if let Some(m) = manifest {
        match fs::read_to_string(m) {
            Ok(text) => Ok(LoadedPatterns::Loaded {
                path: m.to_path_buf(),
                patterns: parse_patterns(&text),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::Usage(format!("manifest {} not found", m.display())))
            }
            Err(e) => Err(Error::io("read manifest", m, e)),
        }
    } else {
        let default_path = root.join(".wtinclude");
        match fs::read_to_string(&default_path) {
            Ok(text) => Ok(LoadedPatterns::Loaded {
                path: default_path,
                patterns: parse_patterns(&text),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LoadedPatterns::Defaults {
                patterns: DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect(),
            }),
            Err(e) => Err(Error::io("read manifest", &default_path, e)),
        }
    }
}

/// Write the starter manifest house-style: temp file beside the
/// destination, then one atomic rename, so a crash never leaves a
/// half-written manifest behind.
pub fn write_starter_manifest(path: &Path) -> Result<()> {
    let refuse =
        |source: std::io::Error| Error::io_unanchored("write starter manifest", path, source);
    let parent = path.parent().ok_or_else(|| {
        Error::Usage("cannot write starter manifest: manifest path has no parent directory".into())
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(refuse)?;
    tmp.write_all(STARTER_MANIFEST.as_bytes()).map_err(refuse)?;
    tmp.persist(path).map_err(|e| refuse(e.error))?;
    Ok(())
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    fn test_matches(pattern: &str, rel: &str) -> bool {
        pattern_matches(pattern, Path::new(rel))
    }

    #[test]
    fn unanchored_pattern_matches_at_any_depth() {
        assert!(test_matches("heavy/", "heavy"));
        assert!(test_matches("heavy/", "a/b/heavy"));
        assert!(test_matches("node_modules", "packages/cli/node_modules"));
        assert!(!test_matches("heavy/", "unheavy"));
        // Per-segment matching: any directory below an already-matched
        // heavy root also matches; collect_matches dedups the nesting.
        assert!(test_matches("heavy/", "heavy/sub"));
    }

    #[test]
    fn anchored_pattern_matches_only_from_root() {
        // Interior slashes make the pattern anchored: it must match
        // starting at the repository root.
        assert!(test_matches("/pkg0*/nested", "pkg01/nested"));
        assert!(!test_matches("/pkg0*/nested", "x/pkg01/nested"));
        assert!(test_matches("/a/b", "a/b"));
        assert!(!test_matches("/a/b", "x/a/b"));
    }

    #[test]
    fn wildcard_segment_matches_partial_names() {
        assert!(test_matches("/pkg0*/nested", "pkg01/nested"));
        assert!(!test_matches("/pkg0*/nested", "pkg11/nested"));
        assert!(test_matches("*-build", "release-build"));
        assert!(!test_matches("*-build", "build"));
    }

    #[test]
    fn double_wildcard_spans_segments() {
        assert!(test_matches("/a/**/b", "a/b"));
        assert!(test_matches("/a/**/b", "a/x/y/z/b"));
        assert!(test_matches("**/generated", "deep/tree/generated"));
        assert!(test_matches("**/generated", "generated"));
        assert!(!test_matches("/a/**/b", "a/x/c"));
    }

    #[test]
    fn empty_and_slash_only_patterns_match_nothing() {
        assert!(!test_matches("/", "anything"));
        assert!(!test_matches("", "anything"));
    }

    #[test]
    fn parse_skips_comments_blanks_and_preserves_negations() {
        let text = "# comment\n\nheavy/\n!keep/\n   \n  target/  \n";
        assert_eq!(parse_patterns(text), vec!["heavy/", "!keep/", "target/"]);
    }

    #[test]
    fn collect_drops_directories_nested_inside_an_earlier_match() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path();
        for dir in ["outer", "outer/inner", "standalone"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let patterns = vec![
            "outer/".to_string(),
            "inner/".to_string(),
            "standalone/".to_string(),
        ];
        let found = collect_matches(root, &patterns).unwrap();
        // `outer/inner` matched `inner/` directly but sits inside the
        // `outer` match, so the outer copy already covers it.
        assert_eq!(
            found,
            vec![PathBuf::from("outer"), PathBuf::from("standalone")]
        );
    }

    #[test]
    fn collect_prunes_git_and_requires_paths_under_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("real")).unwrap();
        let patterns = vec!["*".to_string()];
        let found = collect_matches(root, &patterns).unwrap();
        assert_eq!(found, vec![PathBuf::from("real")]);
    }

    #[test]
    fn missing_explicit_manifest_is_an_error_and_default_uses_in_memory_defaults() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path();

        let err = load_patterns(root, Some(&root.join("nope.wtinclude"))).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");

        let defaults = match load_patterns(root, None).unwrap() {
            LoadedPatterns::Defaults { patterns } => {
                assert_eq!(
                    patterns,
                    DEFAULT_PATTERNS
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                );
                patterns
            }
            LoadedPatterns::Loaded { .. } => panic!("expected in-memory defaults"),
        };
        // Manifest must NOT be written automatically
        assert!(!root.join(".wtinclude").exists());

        // Explicit starter manifest writing
        write_starter_manifest(&root.join(".wtinclude")).unwrap();
        assert!(root.join(".wtinclude").is_file());

        let loaded = load_patterns(root, None).unwrap();
        match loaded {
            LoadedPatterns::Loaded { patterns, path } => {
                assert_eq!(path, root.join(".wtinclude"));
                assert_eq!(patterns, defaults);
            }
            LoadedPatterns::Defaults { .. } => panic!("expected loaded manifest"),
        }
    }

    #[test]
    fn test_is_volatile_cache() {
        assert!(is_volatile_cache("target/debug/incremental"));
        assert!(is_volatile_cache("target/debug/incremental/cache.db"));
        assert!(is_volatile_cache("target/release/incremental/app/o.bin"));
        assert!(is_volatile_cache("crates/sub/target/debug/incremental/o"));
        assert!(is_volatile_cache("node_modules/.vite"));
        assert!(is_volatile_cache("node_modules/.vite/deps/react.js"));
        assert!(is_volatile_cache(
            "frontend/node_modules/.vite/deps/chunk.js"
        ));
        assert!(is_volatile_cache(".next/cache"));
        assert!(is_volatile_cache(".next/cache/webpack/client.pack"));
        assert!(is_volatile_cache("web/.next/cache/turbopack/module.pack"));

        assert!(!is_volatile_cache("target/debug/deps/libfoo.rlib"));
        assert!(!is_volatile_cache("target/release/build/foo"));
        assert!(!is_volatile_cache("node_modules/react/index.js"));
        assert!(!is_volatile_cache("node_modules/vite/bin/vite.js"));
        assert!(!is_volatile_cache(".next/server/pages/index.js"));
        assert!(!is_volatile_cache(".venv/bin/python"));
    }

    #[test]
    fn test_zero_savings_reason_formatting() {
        let no_dirs = ZeroSavingsReason::NoMatchingDirectories;
        assert_eq!(
            no_dirs.explanation(),
            "no matching heavy directories found and the worktree relies strictly on git tracking"
        );
        assert_eq!(
            no_dirs.human_notice(),
            "nothing to hydrate (no matching heavy directories found and the worktree relies strictly on git tracking)"
        );
        assert_eq!(
            no_dirs.human_summary(),
            "0 bytes saved (no matching heavy directories found and the worktree relies strictly on git tracking)"
        );
        assert_eq!(ZeroSavingsReason::DIAGNOSTIC_CODE, "ZERO_SAVINGS");

        let no_files = ZeroSavingsReason::NoFilesHydrated;
        assert_eq!(
            no_files.explanation(),
            "no files hydrated from matching directories and the worktree relies strictly on git tracking"
        );
        assert_eq!(
            no_files.human_notice(),
            "nothing to hydrate (no files hydrated from matching directories and the worktree relies strictly on git tracking)"
        );
        assert_eq!(
            no_files.human_summary(),
            "0 bytes saved (no files hydrated from matching directories and the worktree relies strictly on git tracking)"
        );

        let defaults = LoadedPatterns::Defaults {
            patterns: vec!["a/".into()],
        };
        assert!(defaults.is_defaults());
        assert_eq!(defaults.patterns(), &["a/".to_string()]);

        let loaded = LoadedPatterns::Loaded {
            path: PathBuf::from("p"),
            patterns: vec!["b/".into()],
        };
        assert!(!loaded.is_defaults());
        assert_eq!(loaded.patterns(), &["b/".to_string()]);
    }
}
