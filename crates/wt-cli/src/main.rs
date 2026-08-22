//! Manifest-driven hydration for `wt create` (tickets 02 + 05) and
//! garbage collection for `wt remove`/`wt sweep` (ticket 06).

mod gc;
mod hydrate;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};

use hydrate::{claim_references, ingest_dir, materialize};

#[derive(Parser)]
#[command(
    name = "wt",
    version,
    about = "Instant git worktrees with heavy directories already hydrated"
)]
struct Cli {
    #[command(subcommand)]
    command: WtCommand,
}

#[derive(Subcommand)]
enum WtCommand {
    /// Create a worktree for NAME (used as the git branch name) and
    /// hydrate the heavy directories listed in the .wtinclude manifest.
    #[command(long_about = "Create a worktree for NAME (used as the git branch \
name) and hydrate the heavy directories listed in the .wtinclude manifest.

Hydrated files are private, fully writable copy-on-write clones of store
blobs (fclonefileat on macOS); they share the store's physical blocks until
first write. Filesystems that refuse clones fall back to plain byte copies.

WT_HARDLINK=1 opts into EXPERIMENTAL hardlinked materialization for maximum
space sharing: linked files share the store's inode, which must be made
read-only, so tools that rewrite hydrated files in place fail loudly with
permission errors. WT_NO_HARDLINK=1 forces byte copies instead.

Blobs are hash-verified once and then trusted while their size and mtime
stay unchanged (a verified-blob ledger beside the store tracks this);
WT_VERIFY=1 forces a full re-hash of every blob on every run for paranoid
verification.")]
    Create {
        /// Branch name; also names the new worktree directory.
        name: String,
        /// Manifest listing heavy directories (gitignore syntax).
        /// Defaults to `.wtinclude` in the repository root.
        #[arg(long)]
        manifest: Option<PathBuf>,
        /// Destination for the new worktree. Defaults to a sibling of
        /// the current repository named `<repo>-<name>`.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Remove a worktree and release the store references its
    /// hydration claimed (recorded in the wt-hydrated.tsv ledger).
    Remove {
        /// Branch name; also names the worktree directory, unless
        /// --dir says otherwise.
        name: String,
        /// Path of the worktree to remove. Defaults to the sibling
        /// `<repo>-<name>` that `wt create` produces.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Delete unreferenced store entries older than --age. Entries a
    /// live worktree references are never touched.
    Sweep {
        /// Minimum age of an unreferenced entry before it may be
        /// deleted (e.g. 0s, 90s, 10m, 24h, 7d). The floor protects
        /// content that is mid-ingestion or awaiting its first
        /// reference.
        #[arg(long, default_value = "7d")]
        age: String,
    },
}

/// Used when no manifest exists yet. Deliberately short and boring:
/// these cover the ecosystems that actually produce untracked bulk.
const DEFAULT_PATTERNS: &[&str] = &[
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".cache/",
    ".venv/",
    "__pycache__/",
];

const STARTER_MANIFEST: &str = "\
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

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("not inside a git repository".into());
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

/// Parse manifest text into patterns, skipping blank lines and
/// `#` comments. Negation (`!`) is not supported; such lines are
/// ignored rather than silently misinterpreted.
fn parse_patterns(text: &str) -> Vec<String> {
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
fn pattern_matches(pattern: &str, rel: &Path) -> bool {
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
fn collect_matches(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
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
            let rel = path.strip_prefix(root).expect("path under root");
            if patterns.iter().any(|p| pattern_matches(p, rel)) {
                matched.push(rel.to_path_buf());
            } else {
                stack.push(path);
            }
        }
    }
    matched.sort();
    matched
        .into_iter()
        .scan(None::<PathBuf>, |prev, rel| {
            let covered = prev.as_ref().is_some_and(|p| rel.starts_with(p));
            if !covered {
                *prev = Some(rel.clone());
            }
            Some((!covered).then_some(rel))
        })
        .flatten()
        .collect()
}

fn load_patterns(root: &Path, manifest: Option<&Path>) -> Result<(Vec<String>, bool), String> {
    let path = match manifest {
        Some(m) => m.to_path_buf(),
        None => root.join(".wtinclude"),
    };
    match fs::read_to_string(&path) {
        Ok(text) => Ok((parse_patterns(&text), false)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if manifest.is_some() {
                return Err(format!("manifest {} not found", path.display()));
            }
            println!(
                "no .wtinclude in {}; using defaults ({})",
                root.display(),
                DEFAULT_PATTERNS.join(" ")
            );
            fs::write(&path, STARTER_MANIFEST)
                .map_err(|e| format!("cannot write starter manifest: {e}"))?;
            println!("wrote starter manifest {}", path.display());
            Ok((
                DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect(),
                true,
            ))
        }
        Err(e) => Err(format!("cannot read manifest {}: {e}", path.display())),
    }
}

fn create(name: &str, manifest: Option<&Path>, dir: Option<&Path>) -> Result<(), String> {
    let root = repo_root()?;
    let dest = match dir {
        Some(d) => d.to_path_buf(),
        None => root
            .parent()
            .ok_or("repository root has no parent")?
            .join(format!(
                "{}-{name}",
                root.file_name()
                    .ok_or("cannot name repository directory")?
                    .to_string_lossy()
            )),
    };
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }

    let added = {
        let mut cmd = Command::new("git");
        cmd.current_dir(&root)
            .args(["worktree", "add", "-b", name])
            .arg(&dest)
            .arg("HEAD");
        run(&mut cmd)
    }
    .or_else(|_| {
        let mut cmd = Command::new("git");
        cmd.current_dir(&root)
            .args(["worktree", "add"])
            .arg(&dest)
            .arg(name);
        run(&mut cmd)
    });
    added?;

    println!(
        "created worktree {} from {}",
        dest.display(),
        root.display()
    );

    let (patterns, _used_defaults) = load_patterns(&root, manifest)?;
    let dirs = collect_matches(&root, &patterns);
    if dirs.is_empty() {
        println!("nothing to hydrate");
        return Ok(());
    }

    let mut store = hydrate::open_store()?;
    let mut total_files = 0usize;
    let mut total_copied = 0usize;
    let mut strategy = "byte-copy";
    for rel in &dirs {
        let src = root.join(rel);
        let ingested = ingest_dir(&mut store, &root, &src)?;
        claim_references(&mut store, &dest, &ingested)?;
        // Ingested paths are repo-relative (they include the heavy
        // directory itself), so materialize against the worktree root.
        let report = materialize(&store, &ingested, &dest)
            .map_err(|e| format!("hydration of {} failed: {e}", rel.display()))?;
        total_files += report.files;
        total_copied += report.copied;
        strategy = report.strategy;
        println!(
            "hydrated {} from {} via store ({} file{})",
            rel.display(),
            src.display(),
            report.files,
            if report.files == 1 { "" } else { "s" }
        );
    }
    // Say plainly what happened to shared content.
    if std::env::var_os("WT_NO_HARDLINK").is_some() {
        println!(
            "hardlink mode off (WT_NO_HARDLINK): wrote byte copies for all {total_files} file(s)"
        );
    } else {
        match (strategy, total_copied) {
            ("hardlink", 0) => println!(
                "experimental hardlink mode (WT_HARDLINK): linked shared inodes for all {total_files} file(s)"
            ),
            ("hardlink", n) => println!(
                "experimental hardlink mode (WT_HARDLINK): hardlinks refused for {n} of {total_files} file(s); wrote byte copies"
            ),
            (_, 0) => {}
            (name, n) => println!(
                "{name} unavailable on this filesystem: wrote byte copies for {n} of {total_files} file(s)"
            ),
        }
    }
    // Persist the verified-blob ledger explicitly: the Drop below is
    // a best-effort backup, but a clean run should leave its
    // verifications behind even if something later fails hard.
    store
        .flush()
        .map_err(|e| format!("cannot update verified-blob ledger: {e}"))?;
    println!(
        "hydration complete: {total_files} file{} through the store",
        {
            if total_files == 1 {
                ""
            } else {
                "s"
            }
        }
    );
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        WtCommand::Create {
            name,
            manifest,
            dir,
        } => create(&name, manifest.as_deref(), dir.as_deref()),
        WtCommand::Remove { name, dir } => gc::remove(&name, dir.as_deref()),
        WtCommand::Sweep { age } => gc::sweep(&age),
    };
    if let Err(msg) = result {
        eprintln!("wt: {msg}");
        std::process::exit(1);
    }
}
