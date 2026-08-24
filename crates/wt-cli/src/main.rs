//! Manifest-driven hydration for `wt create` (tickets 02 + 05) and
//! garbage collection for `wt remove`/`wt sweep` (ticket 06).

mod gc;
mod hydrate;
mod manifest;
mod snapshots;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use clap::{Parser, Subcommand};

use wt_store::ContentId;

use hydrate::{
    claim_references, claim_snapshot_references, ingest_dir, materialize, publish_mirror,
    snapshots_enabled, Ingested,
};
use manifest::{collect_matches, load_patterns, pattern_matches, LoadedPatterns};
use snapshots::Outcome as SnapshotOutcome;

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
verification.

GC bookkeeping: each successful create publishes one store-local mirror
(<store>/worktrees/) naming the blobs it hydrates from. WT_TIMING=1
prints per-stage timings (`wt-stage ingest=...` and friends) to stderr.

WT_SNAPSHOTS=1 (macOS/APFS, opt-in) hydrates each heavy directory by
one recursive clonefile(2) from a whole-directory snapshot in the store
when one matches: hits cost no per-file work. Misses build and publish
a snapshot first. WT_VERIFY=1 bypasses snapshot hits entirely and
rebuilds from freshly hashed blobs. Filesystems without clone support,
and clone refusals like cross-device destinations, fall back to the
per-file ladder above.")]
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
    /// Delete store entries no live worktree references and older
    /// than --age. Entries a live worktree references are never
    /// touched. In mark-sweep mode (see `wt store migrate`) liveness
    /// comes from store mirrors plus the grace period instead of
    /// refcounts.
    Sweep {
        /// Minimum age of an unreferenced entry before it may be
        /// deleted (e.g. 0s, 90s, 10m, 24h, 7d). The floor protects
        /// content that is mid-ingestion or awaiting its first
        /// reference. Defaults to 7d in legacy mode, and to
        /// WT_GC_GRACE (default 15m) in mark-sweep mode.
        #[arg(long)]
        age: Option<String>,
    },
    /// Store-level inspection and one-way migrations.
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
}

#[derive(Subcommand)]
enum StoreAction {
    /// Migrate the store's garbage-collection scheme (one-way; see
    /// ADR-0004). Until activated, sweep stays refcount-driven and
    /// every sweep audits mirrors against refs for parity.
    Migrate {
        /// Sweep collects from live-mirror marks plus the grace
        /// period (WT_GC_GRACE, default 15m) from now on. Legacy
        /// refs/ files stay maintained by create/remove so pre-change
        /// binaries remain safe, but are ignored for liveness.
        #[arg(long)]
        activate_mark_sweep: bool,
        /// Drop ALL legacy refcount files and stop writing new ones.
        /// Pre-cutover binaries must not use this store afterwards;
        /// this is loud, explicit, and irreversible.
        #[arg(long)]
        drop_legacy_refs: bool,
    },
}

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

    let timing = std::env::var_os("WT_TIMING").is_some();
    // total spans git-worktree-add through summary printing (Step 0:
    // the git worktree add itself gets its own stage line).
    let started = Instant::now();
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
    let git_worktree_ms = started.elapsed().as_millis();

    println!(
        "created worktree {} from {}",
        dest.display(),
        root.display()
    );

    let mut ingest_ms = 0u128;
    let mut references_ms = 0u128;
    let mut materialize_ms = 0u128;
    let mut snapshot_ms = 0u128;
    let mut snapshot_engaged = false;
    // Step 0: fine-grained sub-stage attribution.
    let mut verify_ms = 0u128;
    let mut place_ms = 0u128;
    let mut snapshot_lookup_ms = 0u128;
    let mut snapshot_clonefile_ms = 0u128;
    let mut build_verify_ms = 0u64;
    let mut build_link_train_ms = 0u64;
    let mut build_publish_ms = 0u64;
    let mut snapshot_built = false;
    // v2 incremental reporting: the mode line shows the LAST heavy
    // directory's serving mode (hit/build/v2); the counters sum.
    let mut snapshot_mode = "build";
    let mut snapshot_v2_cloned = 0usize;
    let mut snapshot_v2_linked = 0usize;
    // One line per stage on stderr (`wt-stage <name>=<ms>`, integer
    // milliseconds). total covers git-worktree-add through the end.
    // The snapshot line appears only when the fast path did work, so
    // pre-snapshot consumers see the same four lines as before; its
    // meaning is unchanged (lookup + build + clone wall time).
    #[allow(clippy::too_many_arguments)]
    let emit = |git_worktree: u128,
                ingest: u128,
                references: u128,
                materialize: u128,
                verify: u128,
                place: u128,
                snapshot: u128,
                engaged: bool,
                lookup: u128,
                clonefile: u128,
                built: Option<(u64, u64, u64)>,
                mode: &str,
                v2_cloned: usize,
                v2_linked: usize| {
        if !timing {
            return;
        }
        eprintln!("wt-stage git-worktree={git_worktree}");
        eprintln!("wt-stage ingest={ingest}");
        eprintln!("wt-stage references={references}");
        eprintln!("wt-stage materialize={materialize}");
        if materialize > 0 {
            eprintln!("wt-stage verify={verify}");
            eprintln!("wt-stage place={place}");
        }
        if engaged {
            eprintln!("wt-stage snapshot={snapshot}");
            eprintln!("wt-stage snapshot-lookup={lookup}");
            eprintln!("wt-stage snapshot-clonefile={clonefile}");
            eprintln!("wt-stage snapshot-mode={mode}");
            eprintln!("wt-stage snapshot-v2-cloned={v2_cloned}");
            eprintln!("wt-stage snapshot-v2-linked={v2_linked}");
            if let Some((bv, blt, bp)) = built {
                eprintln!("wt-stage snapshot-build-verify={bv}");
                eprintln!("wt-stage snapshot-build-link-train={blt}");
                eprintln!("wt-stage snapshot-build-publish={bp}");
            }
        }
        eprintln!("wt-stage total={}", started.elapsed().as_millis());
    };

    let patterns = match load_patterns(&root, manifest)? {
        LoadedPatterns::CreatedStarter { path, patterns } => {
            println!(
                "no .wtinclude in {}; using defaults ({})",
                root.display(),
                manifest::DEFAULT_PATTERNS.join(" ")
            );
            println!("wrote starter manifest {}", path.display());
            patterns
        }
        LoadedPatterns::Loaded { patterns } => patterns,
    };
    let dirs = collect_matches(&root, &patterns)?;
    if dirs.is_empty() {
        println!("nothing to hydrate");
        emit(
            git_worktree_ms,
            0,
            0,
            0,
            0,
            0,
            0,
            false,
            0,
            0,
            None,
            "build",
            0,
            0,
        );
        return Ok(());
    }

    let mut store = hydrate::open_store()?;
    let paranoid = std::env::var_os("WT_VERIFY").is_some();
    let snapshot_gate = snapshots_enabled();
    let mut total_files = 0usize;
    let mut total_copied = 0usize;
    let mut strategy = "byte-copy";
    let mut combined = Ingested {
        dirs: Vec::new(),
        files: BTreeMap::new(),
        symlinks: BTreeMap::new(),
        modes: BTreeMap::new(),
    };
    // Ticket 08: heavy directories hydrated through snapshots record
    // their manifest hashes here; the mirror names them, not the
    // child blobs (the manifest marks those).
    let mut snapshot_hashes: Vec<ContentId> = Vec::new();
    let mut git_dir = dest.clone(); // replaced by claim_references
    for rel in &dirs {
        let src = root.join(rel);
        let stage = Instant::now();
        let ingested = ingest_dir(&mut store, &root, &src)?;
        ingest_ms += stage.elapsed().as_millis();
        let heavy = rel.to_string_lossy().into_owned();

        if snapshot_gate {
            let stage = Instant::now();
            // v2 selection-index key: the first manifest pattern that
            // matched this heavy directory. Only stability across
            // runs matters, not uniqueness.
            let pattern = patterns
                .iter()
                .find(|p| pattern_matches(p, rel))
                .map(String::as_str)
                .unwrap_or("");
            match snapshots::hydrate(
                &mut store, &ingested, &root, pattern, &src, &heavy, &dest, paranoid,
            ) {
                SnapshotOutcome::Hydrated(h) => {
                    snapshot_ms += stage.elapsed().as_millis();
                    snapshot_engaged = true;
                    snapshot_lookup_ms += h.lookup_ms;
                    snapshot_clonefile_ms += h.clonefile_ms;
                    snapshot_mode = h.mode;
                    snapshot_v2_cloned += h.cloned_units;
                    snapshot_v2_linked += h.linked_files;
                    if let Some(b) = h.build {
                        snapshot_built = true;
                        build_verify_ms += b.verify_ms;
                        build_link_train_ms += b.link_train_ms;
                        build_publish_ms += b.publish_ms;
                    }
                    let refs = Instant::now();
                    git_dir = claim_snapshot_references(&mut store, &dest, &ingested, h.hash)?;
                    references_ms += refs.elapsed().as_millis();
                    snapshot_hashes.push(h.hash);
                    total_files += h.files;
                    println!(
                        "hydrated {heavy} from {} via snapshot {} (one clone, {} file{})",
                        src.display(),
                        &h.hash.to_string()[..12],
                        h.files,
                        if h.files == 1 { "" } else { "s" },
                    );
                    continue;
                }
                SnapshotOutcome::FellBack(Some(reason)) => {
                    eprintln!(
                        "wt-snapshots: {heavy}: falling back to per-file placement ({reason})"
                    );
                }
                SnapshotOutcome::FellBack(None) => {}
                SnapshotOutcome::Failed(msg) => {
                    return Err(format!("hydration of {heavy} failed: {msg}"))
                }
            }
            // Fell through to the per-file ladder below; its cost is
            // counted by the stages it runs itself.
        }

        let stage = Instant::now();
        git_dir = claim_references(&mut store, &dest, &ingested)?;
        references_ms += stage.elapsed().as_millis();
        // Ingested paths are repo-relative (they include the heavy
        // directory itself), so materialize against the worktree root.
        let stage = Instant::now();
        let report = materialize(&store, &ingested, &dest)
            .map_err(|e| format!("hydration of {} failed: {e}", rel.display()))?;
        materialize_ms += stage.elapsed().as_millis();
        verify_ms += report.verify_ms;
        place_ms += report.place_ms;
        combined.dirs.extend(ingested.dirs.iter().cloned());
        for (rel, id) in &ingested.files {
            combined.files.insert(rel.clone(), *id);
        }
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
    // Ticket 07: one atomic mirror write per successful create is
    // the GC bookkeeping mark-and-sweep marks through. Ticket 08:
    // snapshot-hydrated dirs appear as `snapshot` records here.
    let stage = Instant::now();
    publish_mirror(&mut store, &dest, &git_dir, &combined, &snapshot_hashes)?;
    references_ms += stage.elapsed().as_millis();
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
    emit(
        git_worktree_ms,
        ingest_ms,
        references_ms,
        materialize_ms,
        verify_ms,
        place_ms,
        snapshot_ms,
        snapshot_engaged,
        snapshot_lookup_ms,
        snapshot_clonefile_ms,
        snapshot_built.then_some((build_verify_ms, build_link_train_ms, build_publish_ms)),
        snapshot_mode,
        snapshot_v2_cloned,
        snapshot_v2_linked,
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
        WtCommand::Sweep { age } => gc::sweep(age.as_deref()),
        WtCommand::Store { action } => match action {
            StoreAction::Migrate {
                activate_mark_sweep,
                drop_legacy_refs,
            } => {
                if activate_mark_sweep == drop_legacy_refs {
                    Err("choose exactly one of --activate-mark-sweep or --drop-legacy-refs".into())
                } else {
                    gc::migrate(activate_mark_sweep, drop_legacy_refs)
                }
            }
        },
    };
    if let Err(msg) = result {
        eprintln!("wt: {msg}");
        std::process::exit(1);
    }
}
