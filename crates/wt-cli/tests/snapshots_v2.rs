// Tests assert with unwrap/expect by design: a panic IS the failure
// signal under test, so the workspace restriction lints stay off here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! v2 diff-based incremental snapshot rebuilds, end to end.
//!
//! Every test runs the real `wt` binary against a real temporary git
//! repository with `WT_SNAPSHOTS=1 AND WT_SNAPSHOTS_V2=1` and an
//! isolated `WT_STORE`. Coverage:
//!
//! - content bump: w2 rebuilds INCREMENTALLY (snapshot-mode=v2,
//!   snapshot-v2-linked well below the file count) and lands a tree
//!   byte-identical to the source
//! - corrupted old-snapshot manifest: v2 selection rejects it and the
//!   create succeeds via a plain full build (snapshot-mode=build)
//! - WT_VERIFY=1 with both gates: exact tree regardless of which mode
//!   served it

// The whole-directory snapshot fast path is macOS/APFS-only (see
// crates/wt-cli/src/snapshots.rs: on other platforms the gate is a
// no-op and no wt-stage snapshot lines are ever printed, so these
// tests would fail rather than skip).
#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;

/// A throwaway git repo whose `heavy/` tree is big enough for the
/// incremental heuristic to bite: 5 generated package dirs (~250
/// files), a symlink, an executable, and an explicit empty dir.
struct V2Fixture {
    repo: PathBuf,
    _base: tempfile::TempDir,
}

const PACKAGES: usize = 5;
const FILES_PER_PACKAGE: usize = 50;

impl V2Fixture {
    fn new() -> V2Fixture {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("origin");
        fs::create_dir_all(&repo).expect("mkdir repo");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test"]);

        let heavy = repo.join("heavy");
        for p in 0..PACKAGES {
            let dir = heavy.join(format!("pkg{p:02}"));
            fs::create_dir_all(&dir).expect("mkdir pkg");
            for i in 0..FILES_PER_PACKAGE {
                fs::write(
                    dir.join(format!("file-{i:03}.txt")),
                    format!("package {p} file {i}\n"),
                )
                .expect("write file");
            }
        }
        let exec = heavy.join("exec.sh");
        fs::write(&exec, "#!/bin/sh\necho hi\n").unwrap();
        set_mode(&exec, 0o755);
        fs::create_dir(heavy.join("empty")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../exec.sh", heavy.join("bin-link")).unwrap();

        fs::write(repo.join(".gitignore"), "heavy/\n").unwrap();
        fs::write(repo.join(".wtinclude"), "heavy/\n").unwrap();
        fs::write(repo.join("src.txt"), "tracked source\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);

        V2Fixture { repo, _base: base }
    }

    fn heavy(&self) -> PathBuf {
        self.repo.join("heavy")
    }

    /// Run `wt` with full environment control.
    fn wt(&self, args: &[&str], env: &[(&str, &str)], worktree_name: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", self.store_path())
            .env_remove("WT_HARDLINK")
            .env_remove("WT_NO_HARDLINK")
            .env_remove("WT_GC_GRACE")
            .envs(env.iter().copied())
            .current_dir(&self.repo)
            .args([
                "--dir",
                &self.worktree_path(worktree_name).to_string_lossy(),
            ])
            .output()
            .expect("run wt binary")
    }

    fn store_path(&self) -> PathBuf {
        self.repo.parent().unwrap().join("isolated-store")
    }

    fn worktree_path(&self, name: &str) -> PathBuf {
        self.repo.parent().unwrap().join(name)
    }

    /// Run `wt` with the isolated store but WITHOUT `--dir`, for
    /// commands that resolve their own context (sweep, migrate,
    /// remove).
    fn wt_raw(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(args)
            .env("WT_STORE", self.store_path())
            .env_remove("WT_HARDLINK")
            .env_remove("WT_NO_HARDLINK")
            .env_remove("WT_GC_GRACE")
            .current_dir(&self.repo)
            .output()
            .expect("run wt binary")
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn assert_created(out: &Output) {
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The whole hydrated tree matches the source: every regular file's
/// bytes, symlink targets, exec bits, and the explicit empty dir.
fn assert_tree_matches_source(source: &Path, hydrated_heavy: &Path) {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let p = entry.expect("dir entry").path();
            if p.symlink_metadata().expect("lstat").is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut src_files = Vec::new();
    walk(source, &mut src_files);
    src_files.sort();
    assert!(!src_files.is_empty());

    for src_path in &src_files {
        let rel = src_path.strip_prefix(source).unwrap();
        let dest_path = hydrated_heavy.join(rel);
        let src_meta = src_path.symlink_metadata().unwrap();
        if src_meta.file_type().is_symlink() {
            let md = dest_path.symlink_metadata().expect("symlink survived");
            assert!(
                md.file_type().is_symlink(),
                "{} must stay a symlink",
                rel.display()
            );
            assert_eq!(
                fs::read_link(&dest_path).unwrap(),
                fs::read_link(src_path).unwrap(),
                "symlink target must match"
            );
        } else {
            let got = fs::read(&dest_path)
                .unwrap_or_else(|e| panic!("cannot read hydrated {}: {e}", rel.display()));
            let want = fs::read(src_path)
                .unwrap_or_else(|e| panic!("cannot read source {}: {e}", rel.display()));
            assert_eq!(got, want, "content mismatch at {}", rel.display());
            if src_meta.mode() & 0o111 != 0 {
                let md = dest_path.metadata().unwrap();
                assert_eq!(
                    md.mode() & 0o111,
                    0o111,
                    "exec bits must survive at {}",
                    rel.display()
                );
            }
        }
    }
    assert!(
        hydrated_heavy.join("empty").is_dir(),
        "empty directory vanished during hydration"
    );
}

/// Count regular files under `dir` (for the linked < total bound).
fn count_files(dir: &Path) -> usize {
    fn walk(dir: &Path, out: &mut usize) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let p = entry.expect("dir entry").path();
            if p.is_dir() && !p.symlink_metadata().unwrap().is_symlink() {
                walk(&p, out);
            } else if !p.symlink_metadata().unwrap().is_symlink() {
                *out += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

/// Mutate ONE existing file and ADD one new package dir with a few
/// files: the minimal bump the incremental path exists for.
fn bump_source(heavy: &Path) {
    fs::write(
        heavy.join("pkg02/file-010.txt"),
        "package 2 file 10 EDITED\n",
    )
    .unwrap();
    let fresh = heavy.join("pkg05");
    fs::create_dir_all(&fresh).unwrap();
    for i in 0..4 {
        fs::write(
            fresh.join(format!("new-{i}.txt")),
            format!("brand new {i}\n"),
        )
        .unwrap();
    }
}

const BOTH_GATES: &[(&str, &str)] = &[("WT_SNAPSHOTS", "1"), ("WT_SNAPSHOTS_V2", "1")];

#[test]
fn bump_rebuilds_incrementally_and_matches_source_exactly() {
    let fx = V2Fixture::new();

    // Seed generation 1 through the gated path.
    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));

    // Generation 2: one edited file + one added package dir.
    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=v2"),
        "the bump must rebuild incrementally:\n{stderr}"
    );

    let linked = stderr
        .lines()
        .find_map(|l| l.strip_prefix("wt-stage snapshot-v2-linked="))
        .map(|v| v.trim().parse::<usize>().expect("integer linked count"));
    let linked = linked.expect("snapshot-v2-linked line must be present");

    // Whole-tree clone + in-place delta: exactly ONE bulk clonefile
    // seeds the rebuild, no matter how the changes are scattered.
    let cloned = stderr
        .lines()
        .find_map(|l| l.strip_prefix("wt-stage snapshot-v2-cloned="))
        .map(|v| v.trim().parse::<usize>().expect("integer cloned count"));
    assert_eq!(cloned, Some(1), "the whole old tree must clone as ONE unit");

    let total_files = count_files(&fx.heavy());
    assert!(
        linked < total_files,
        "incremental rebuild hardlinked {linked} of {total_files} files — \
         that is not incremental"
    );

    // Exact fidelity regardless of how it was served.
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}

#[test]
fn corrupt_old_manifest_falls_back_to_full_build() {
    let fx = V2Fixture::new();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));

    // Corrupt (truncate into garbage) the only published snapshot's
    // manifest: v2 selection must reject it as unusable.
    let snapshots_dir = fx.store_path().join("snapshots");
    let hash = fs::read_dir(&snapshots_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .find(|n| wt_store::ContentId::from_hex(n).is_some())
        .expect("published snapshot");
    fs::write(snapshots_dir.join(&hash).join("manifest.tsv"), "garbage\n").unwrap();

    // The bump changes content anyway, so the rebuilt snapshot lands
    // on a FRESH address rather than colliding with the debris.
    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "with no usable old snapshot the create must take the plain \
         full build:\n{stderr}"
    );
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}

#[test]
fn paranoid_verify_with_v2_gates_still_lands_an_exact_tree() {
    let fx = V2Fixture::new();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));

    // Bump content so the second create cannot just hit; run it
    // paranoid WITH both gates. Whatever mode serves it, the landed
    // tree must be exact.
    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[
            ("WT_SNAPSHOTS", "1"),
            ("WT_SNAPSHOTS_V2", "1"),
            ("WT_VERIFY", "1"),
            ("WT_TIMING", "1"),
        ],
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode="),
        "mode line must be emitted whenever the snapshot path engaged:\n{stderr}"
    );
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}

// ---- crash safety, GC interaction, index resilience --------------------

/// Hex-named directories under `<store>/snapshots/` — the published
/// (or debris) snapshot addresses. tmp/ and the selection index are
/// not addresses.
fn published_hashes(store: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(store.join("snapshots")) else {
        // Nothing published yet (an early kill may not have gotten
        // as far as creating the snapshots directory).
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        // Publish addresses are exactly 64 lowercase hex chars. Anything
        // else under snapshots/ is debris by construction (`tmp/`,
        // `index.tsv`, index-save temp files killed mid-rename) and is
        // swept elsewhere; only real addresses must pass validity here.
        .filter(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()))
        .collect();
    out.sort();
    out
}

fn manifest_of(store: &Path, hash: &str) -> wt_store::Manifest {
    let id = wt_store::ContentId::from_hex(hash).expect("hex hash");
    let text =
        fs::read_to_string(wt_store::snapshot_path(store, &id).join("manifest.tsv")).unwrap();
    wt_store::Manifest::parse(&text).expect("valid manifest")
}

fn blob_ids(manifest: &wt_store::Manifest) -> BTreeSet<wt_store::ContentId> {
    manifest.entries.iter().filter_map(|e| e.blob).collect()
}

fn object_path(store: &Path, id: &wt_store::ContentId) -> PathBuf {
    let hex = id.to_string();
    store.join("objects").join(&hex[..2]).join(&hex[2..])
}

/// After every kill: anything sitting at a published address must
/// pass THE shared validity check (`read_published` is what lookup,
/// v2 selection, and GC all go through). A directory that looks
/// published but lacks `.complete` or carries a torn manifest would
/// mean a non-atomic publish escaped into the wild.
fn assert_no_incomplete_published(store: &Path, iteration: usize) {
    for name in published_hashes(store) {
        let hash = wt_store::ContentId::from_hex(&name).expect("hex address");
        assert!(
            wt_store::read_published_snapshot(store, &hash).is_some(),
            "kill iteration {iteration}: {name} sits at a published \
             address but fails the shared validity check"
        );
    }
}

/// Kill delays swept across the phases of one create: ingest,
/// verify/link, publish, clonefile-out, mirror/index bookkeeping.
const KILL_DELAYS_MS: &[u64] = &[10, 60, 120, 180, 240, 300, 350, 400];

#[cfg(target_os = "macos")]
#[test]
fn sigkilled_creates_never_leave_a_half_published_snapshot_behind() {
    let fx = V2Fixture::new();

    for (i, delay_ms) in KILL_DELAYS_MS.iter().enumerate() {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wt"))
            .args(["create", &format!("killed-{i}")])
            .arg("--dir")
            .arg(fx.worktree_path(&format!("origin-killed-{i}")))
            .env("WT_STORE", fx.store_path())
            .envs(BOTH_GATES.iter().copied())
            .env_remove("WT_HARDLINK")
            .env_remove("WT_NO_HARDLINK")
            .env_remove("WT_GC_GRACE")
            .current_dir(&fx.repo)
            .spawn()
            .expect("spawn wt create");
        thread::sleep(Duration::from_millis(*delay_ms));
        // SIGKILL on Unix; a no-op when the create already finished.
        let _ = child.kill();
        let _ = child.wait();

        // Invariant 1: no torn directory at any published address.
        assert_no_incomplete_published(&fx.store_path(), i);

        // Invariant 2: the next run treats whatever it finds as a hit
        // or a miss, never as a usable half-snapshot, and lands an
        // exactly-correct tree anyway.
        let out = fx.wt(
            &["create", &format!("after-{i}")],
            BOTH_GATES,
            &format!("origin-after-{i}"),
        );
        assert_created(&out);
        assert_tree_matches_source(
            &fx.heavy(),
            &fx.worktree_path(&format!("origin-after-{i}")).join("heavy"),
        );
    }
}

/// Two generations sharing almost every blob. Only worktree two's
/// mirror stays live, referencing ONLY B. Sweep may collect A's
/// directory and A's exclusive blobs, but every blob SHARED with B
/// survives because B's manifest marks them — and the selection
/// index must come through untouched.
#[test]
fn sweep_evicts_unreferenced_old_generation_but_shared_blobs_survive() {
    let fx = V2Fixture::new();
    let store = fx.store_path();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));
    let before = published_hashes(&store);
    assert_eq!(before.len(), 1);
    let a_hash = before[0].clone();

    bump_source(&fx.heavy());
    assert_created(&fx.wt(&["create", "two"], BOTH_GATES, "origin-two"));
    let after = published_hashes(&store);
    assert_eq!(after.len(), 2);
    let b_hash = after
        .iter()
        .find(|h| **h != a_hash)
        .expect("the second generation published under a fresh hash");

    let a_blobs = blob_ids(&manifest_of(&store, &a_hash));
    let b_blobs = blob_ids(&manifest_of(&store, b_hash));
    let shared: Vec<_> = a_blobs.intersection(&b_blobs).copied().collect();
    let exclusive_a: Vec<_> = a_blobs.difference(&b_blobs).copied().collect();
    let exclusive_b: Vec<_> = b_blobs.difference(&a_blobs).copied().collect();
    assert!(!shared.is_empty(), "fixture must actually share blobs");
    assert!(!exclusive_a.is_empty(), "the mutated file's old blob");
    assert!(!exclusive_b.is_empty(), "the bump's new blobs");

    // Only B stays live: removing worktree one retires its mirror.
    let removed = fx.wt_raw(&["remove", "one"]);
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );

    assert!(
        fx.wt_raw(&["store", "migrate", "--activate-mark-sweep"])
            .status
            .success()
    );
    let swept = fx.wt_raw(&["sweep", "--age", "0s"]);
    assert!(
        swept.status.success(),
        "{}",
        String::from_utf8_lossy(&swept.stderr)
    );

    assert!(
        !wt_store::snapshot_path(&store, &wt_store::ContentId::from_hex(&a_hash).unwrap()).exists(),
        "unreferenced old generation must be evicted"
    );
    assert!(
        wt_store::snapshot_path(&store, &wt_store::ContentId::from_hex(b_hash).unwrap()).exists(),
        "referenced current generation must survive"
    );
    for id in &exclusive_a {
        assert!(
            !object_path(&store, id).exists(),
            "A-exclusive blob {id} should be collected"
        );
    }
    for id in shared.iter().chain(exclusive_b.iter()) {
        assert!(
            object_path(&store, id).is_file(),
            "blob {id} is marked by B's manifest and must survive"
        );
    }

    // Regression: sweep_snapshots used to take the selection index
    // with the rest of its non-hex debris, silently killing v2
    // incremental selection store-wide.
    assert!(
        store.join("snapshots/index.tsv").is_file(),
        "sweep deleted the v2 selection index"
    );
    let index = fs::read_to_string(store.join("snapshots/index.tsv")).unwrap();
    assert!(
        index.contains(b_hash),
        "selection index lost the current generation's ring entry"
    );
}

/// Mirror referencing A while B also exists; B's directory is then
/// destroyed out-of-band. Sweep keeps A and every blob A's manifest
/// marks; only B's exclusive content becomes collectable.
#[test]
fn sweep_keeps_old_generation_alive_when_newer_snapshot_dir_vanishes() {
    let fx = V2Fixture::new();
    let store = fx.store_path();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));
    let a_hash = published_hashes(&store)[0].clone();

    bump_source(&fx.heavy());
    assert_created(&fx.wt(&["create", "two"], BOTH_GATES, "origin-two"));
    let b_hash = published_hashes(&store)
        .into_iter()
        .find(|h| *h != a_hash)
        .unwrap();

    let a_manifest = manifest_of(&store, &a_hash);
    let b_manifest = manifest_of(&store, &b_hash);
    let exclusive_b: Vec<_> = blob_ids(&b_manifest)
        .difference(&blob_ids(&a_manifest))
        .copied()
        .collect();
    assert!(!exclusive_b.is_empty());

    // Both mirrors stay live (both worktrees exist). B's record is
    // about to become unresolvable, which GC must treat as "marks
    // nothing", never as fatal.
    fs::remove_dir_all(wt_store::snapshot_path(
        &store,
        &wt_store::ContentId::from_hex(&b_hash).unwrap(),
    ))
    .unwrap();

    assert!(
        fx.wt_raw(&["store", "migrate", "--activate-mark-sweep"])
            .status
            .success()
    );
    let swept = fx.wt_raw(&["sweep", "--age", "0s"]);
    assert!(
        swept.status.success(),
        "an unresolvable snapshot reference must not break sweep: {}",
        String::from_utf8_lossy(&swept.stderr)
    );

    assert!(
        wt_store::snapshot_path(&store, &wt_store::ContentId::from_hex(&a_hash).unwrap()).is_dir(),
        "A is referenced by a live mirror and must survive"
    );
    for entry in &a_manifest.entries {
        if let Some(id) = entry.blob {
            assert!(
                object_path(&store, &id).is_file(),
                "blob {id} of referenced snapshot A was collected"
            );
        }
    }
    for id in &exclusive_b {
        assert!(
            !object_path(&store, id).exists(),
            "B-exclusive blob {id} lost its only mark and should be collected"
        );
    }
}

/// The tree inside an incrementally published snapshot holds real
/// bytes: unit-cloned files are private inodes with correct content,
/// not references into the old generation.
#[test]
fn incremental_snapshot_tree_files_are_private_and_self_contained() {
    let fx = V2Fixture::new();
    let store = fx.store_path();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));
    let a_hash = published_hashes(&store)[0].clone();
    bump_source(&fx.heavy());
    assert_created(&fx.wt(&["create", "two"], BOTH_GATES, "origin-two"));
    let b_hash = published_hashes(&store)
        .into_iter()
        .find(|h| *h != a_hash)
        .unwrap();

    // pkg01 stayed byte-identical across the bump, so its subtree
    // arrived in B by one recursive clonefile out of A's tree.
    let sample =
        wt_store::snapshot_tree_path(&store, &wt_store::ContentId::from_hex(&b_hash).unwrap())
            .join("pkg01/file-000.txt");
    assert_eq!(
        fs::read_to_string(&sample).unwrap(),
        "package 1 file 0\n",
        "cloned-unit content must be exact"
    );

    #[cfg(target_os = "macos")]
    {
        let md = fs::symlink_metadata(&sample).unwrap();
        assert_eq!(
            md.nlink(),
            1,
            "a cloned unit file must own a private inode, not a link \
             into the old tree or the blob store"
        );
        assert!(md.is_file(), "must be a regular file, not a symlink");
    }

    // Not a dangling reference into the old generation: destroy A
    // wholesale and B's bytes still read back.
    fs::remove_dir_all(wt_store::snapshot_path(
        &store,
        &wt_store::ContentId::from_hex(&a_hash).unwrap(),
    ))
    .unwrap();
    assert_eq!(
        fs::read_to_string(&sample).unwrap(),
        "package 1 file 0\n",
        "B's cloned files must not depend on A's directory"
    );
}

/// The selection index is pure optimization metadata. With it made
/// permanently unusable (a DIRECTORY squatting on index.tsv defeats
/// both load and save), a create must degrade to the plain full
/// build and still land an exact tree.
#[test]
fn unusable_selection_index_never_fails_the_create() {
    let fx = V2Fixture::new();

    assert_created(&fx.wt(&["create", "one"], BOTH_GATES, "origin-one"));
    let index = fx.store_path().join("snapshots/index.tsv");
    let journal = fx.store_path().join("snapshots/journal.tsv");
    if index.is_file() {
        let _ = fs::remove_file(&index);
    }
    fs::create_dir_all(&index).expect("squat a directory on index.tsv");
    if journal.is_file() {
        let _ = fs::remove_file(&journal);
    }
    fs::create_dir_all(&journal).expect("squat a directory on journal.tsv");

    bump_source(&fx.heavy());
    let out = fx.wt(
        &["create", "two"],
        &[BOTH_GATES, &[("WT_TIMING", "1")]].concat(),
        "origin-two",
    );
    assert_created(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("wt-stage snapshot-mode=build"),
        "no usable selection means the plain full build:\n{stderr}"
    );
    assert_tree_matches_source(&fx.heavy(), &fx.worktree_path("origin-two").join("heavy"));
}
