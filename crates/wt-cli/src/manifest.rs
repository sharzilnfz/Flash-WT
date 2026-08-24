//! The `.wtinclude` manifest (tickets 02 + 05, decomposed from main.rs
//! by arch-hardening ticket 03): parsing, gitignore-style pattern
//! matching against repo-relative directory paths, directory discovery,
//! and starter-manifest creation.
//!
//! Everything here is pure filesystem-and-strings logic, unit-tested in
//! place — no binary spawn required.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Used when no manifest exists yet. Deliberately short and boring:
/// these cover the ecosystems that actually produce untracked bulk.
pub const DEFAULT_PATTERNS: &[&str] = &[
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".cache/",
    ".venv/",
    "__pycache__/",
];

pub const STARTER_MANIFEST: &str = "\
# wt: directories hydrated into every new worktree.
# Gitignore syntax, relative to this repository root. Edit freely;
# anything listed here is copied (never moved) from this checkout.
node_modules/
target/
dist/
build/
.cache/
.venv/
__pycache__/
";

/// What [`load_patterns`] decided. Splitting the decision from the
/// printing keeps this module side-effect-free above the filesystem:
/// the caller owns user-visible output.
#[derive(Debug)]
pub enum LoadedPatterns {
    /// An existing manifest was read.
    Loaded { patterns: Vec<String> },
    /// No default-path manifest existed; defaults were chosen and a
    /// starter manifest written to `path`.
    CreatedStarter {
        path: PathBuf,
        patterns: Vec<String>,
    },
}

/// Parse manifest text into patterns, skipping blank lines and
/// `#` comments. Negation (`!`) is not supported; such lines are
/// ignored rather than silently misinterpreted.
pub fn parse_patterns(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('!'))
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
    let path_segs: Vec<&str> = rel_text.split('/').collect();
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
            if patterns.iter().any(|p| pattern_matches(p, rel)) {
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
/// manifest is not an error — defaults apply and a starter manifest
/// is written atomically (temp file beside the destination, one
/// rename); the caller prints the announcements.
pub fn load_patterns(root: &Path, manifest: Option<&Path>) -> Result<LoadedPatterns> {
    let path = match manifest {
        Some(m) => m.to_path_buf(),
        None => root.join(".wtinclude"),
    };
    match fs::read_to_string(&path) {
        Ok(text) => Ok(LoadedPatterns::Loaded {
            patterns: parse_patterns(&text),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if manifest.is_some() {
                return Err(Error::Usage(format!(
                    "manifest {} not found",
                    path.display()
                )));
            }
            write_starter_manifest(&path)?;
            Ok(LoadedPatterns::CreatedStarter {
                path,
                patterns: DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect(),
            })
        }
        Err(e) => Err(Error::io("read manifest", &path, e)),
    }
}

/// Write the starter manifest house-style: temp file beside the
/// destination, then one atomic rename, so a crash never leaves a
/// half-written manifest behind.
fn write_starter_manifest(path: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, rel: &str) -> bool {
        pattern_matches(pattern, Path::new(rel))
    }

    #[test]
    fn unanchored_pattern_matches_at_any_depth() {
        assert!(matches("heavy/", "heavy"));
        assert!(matches("heavy/", "a/b/heavy"));
        assert!(matches("node_modules", "packages/cli/node_modules"));
        assert!(!matches("heavy/", "unheavy"));
        // Per-segment matching: any directory below an already-matched
        // heavy root also matches; collect_matches dedups the nesting.
        assert!(matches("heavy/", "heavy/sub"));
    }

    #[test]
    fn anchored_pattern_matches_only_from_root() {
        // Interior slashes make the pattern anchored: it must match
        // starting at the repository root.
        assert!(matches("/pkg0*/nested", "pkg01/nested"));
        assert!(!matches("/pkg0*/nested", "x/pkg01/nested"));
        assert!(matches("/a/b", "a/b"));
        assert!(!matches("/a/b", "x/a/b"));
    }

    #[test]
    fn wildcard_segment_matches_partial_names() {
        assert!(matches("/pkg0*/nested", "pkg01/nested"));
        assert!(!matches("/pkg0*/nested", "pkg11/nested"));
        assert!(matches("*-build", "release-build"));
        assert!(!matches("*-build", "build"));
    }

    #[test]
    fn double_wildcard_spans_segments() {
        assert!(matches("/a/**/b", "a/b"));
        assert!(matches("/a/**/b", "a/x/y/z/b"));
        assert!(matches("**/generated", "deep/tree/generated"));
        assert!(matches("**/generated", "generated"));
        assert!(!matches("/a/**/b", "a/x/c"));
    }

    #[test]
    fn empty_and_slash_only_patterns_match_nothing() {
        assert!(!matches("/", "anything"));
        assert!(!matches("", "anything"));
    }

    #[test]
    fn parse_skips_comments_blanks_and_negations() {
        let text = "# comment\n\nheavy/\n!keep/\n   \n  target/  \n";
        assert_eq!(parse_patterns(text), vec!["heavy/", "target/"]);
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
    fn missing_explicit_manifest_is_an_error_but_default_creates_starter() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path();

        let err = load_patterns(root, Some(&root.join("nope.wtinclude"))).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");

        let starter_path = match load_patterns(root, None).unwrap() {
            LoadedPatterns::CreatedStarter { path, patterns } => {
                assert_eq!(
                    patterns,
                    DEFAULT_PATTERNS
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
                );
                path
            }
            LoadedPatterns::Loaded { .. } => panic!("expected starter creation"),
        };
        assert_eq!(starter_path, root.join(".wtinclude"));
        // The written starter parses back to the default patterns.
        let again = load_patterns(root, None).unwrap();
        assert!(matches!(again, LoadedPatterns::Loaded { .. }));
    }
}
