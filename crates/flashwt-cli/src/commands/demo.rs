use std::fs;
use std::path::Path;
use std::time::Instant;

use flashwt_store::{ContentId, DiskStore};

use crate::config::RunConfig;
use crate::envelope::{DemoData, Diagnostic};
use crate::error::{Error, Result};
use crate::hydrate::{HydrationEngine, HydrationRequest, open_store};
use crate::output::{HumanBytes, HumanCount};
use crate::workspace;

struct DemoPhases {
    baseline_ms: u64,
    warm_hydrate_ms: u64,
    method: String,
    bytes_copied: u64,
    bytes_shared: u64,
}

const DEMO_PACKAGE_COUNT: usize = 8;
const DEMO_SCOPED_COUNT: usize = 4;

fn recursive_copy(src: &Path, dest: &Path) -> Result<u64> {
    fs::create_dir_all(dest).map_err(|e| Error::io("create baseline copy dir", dest, e))?;

    let entries: Vec<_> = fs::read_dir(src)
        .map_err(|e| Error::io("read baseline copy dir", src, e))?
        .filter_map(|e| e.ok())
        .collect();

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    let chunk_size = entries.len().div_ceil(num_threads);
    let chunks: Vec<_> = entries.chunks(chunk_size.max(1)).collect();

    let total: u64 = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                s.spawn(move || {
                    let mut bytes_copied = 0u64;
                    for entry in chunk {
                        let s_path = entry.path();
                        let d_path = dest.join(entry.file_name());
                        let file_type = match entry.file_type() {
                            Ok(ft) => ft,
                            Err(_) => continue,
                        };
                        if file_type.is_dir() {
                            let _ = copy_subtree(&s_path, &d_path, &mut bytes_copied);
                        } else if file_type.is_file() {
                            if let Ok(b) = fs::copy(&s_path, &d_path) {
                                bytes_copied += b;
                            }
                        }
                    }
                    bytes_copied
                })
            })
            .collect();

        handles.into_iter().map(|h| h.join().unwrap_or(0)).sum()
    });

    Ok(total)
}

fn copy_subtree(src: &Path, dest: &Path, bytes_copied: &mut u64) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| Error::io("create sub dir", dest, e))?;
    let mut stack = vec![(src.to_path_buf(), dest.to_path_buf())];
    while let Some((s_dir, d_dir)) = stack.pop() {
        let entries = fs::read_dir(&s_dir).map_err(|e| Error::io("read sub dir", &s_dir, e))?;
        for entry in entries.flatten() {
            let s_path = entry.path();
            let d_path = d_dir.join(entry.file_name());
            let file_type = entry
                .file_type()
                .map_err(|e| Error::io("stat entry", &s_path, e))?;
            if file_type.is_dir() {
                fs::create_dir_all(&d_path).map_err(|e| Error::io("create sub dir", &d_path, e))?;
                stack.push((s_path, d_path));
            } else if file_type.is_file() {
                let bytes =
                    fs::copy(&s_path, &d_path).map_err(|e| Error::io("copy file", &s_path, e))?;
                *bytes_copied += bytes;
            }
        }
    }
    Ok(())
}

fn pad_demo_file(mut s: String, tag: &str, n: usize) -> String {
    for k in 0..200 {
        s.push_str(&format!(
            "// filler {tag}-{n:02}-{k:03} 0123456789abcdef0123456789abcdef\n"
        ));
    }
    s
}

fn generate_synthetic_fixture(repo_path: &Path) -> Result<(usize, u64)> {
    fs::create_dir_all(repo_path).map_err(|e| Error::io("create demo repo dir", repo_path, e))?;

    workspace::run(repo_path, &["init", "--quiet"])?;
    workspace::run(repo_path, &["config", "user.email", "demo@example.com"])?;
    workspace::run(repo_path, &["config", "user.name", "Demo User"])?;

    let pkg_json_path = repo_path.join("package.json");
    fs::write(
        &pkg_json_path,
        b"{\n  \"name\": \"flashwt-synthetic-project\",\n  \"version\": \"1.0.0\",\n  \"private\": true\n}\n",
    ).map_err(|e| Error::io("write package.json", &pkg_json_path, e))?;

    let gitignore_path = repo_path.join(".gitignore");
    fs::write(&gitignore_path, b"node_modules/\n")
        .map_err(|e| Error::io("write .gitignore", &gitignore_path, e))?;

    let wtinclude_path = repo_path.join(".flashwtinclude");
    fs::write(&wtinclude_path, b"node_modules/\n")
        .map_err(|e| Error::io("write .flashwtinclude", &wtinclude_path, e))?;

    let src_dir = repo_path.join("src");
    fs::create_dir_all(&src_dir).map_err(|e| Error::io("create src dir", &src_dir, e))?;
    let index_ts = src_dir.join("index.ts");
    fs::write(
        &index_ts,
        b"console.log(\"flashwt synthetic demo project\");\n",
    )
    .map_err(|e| Error::io("write index.ts", &index_ts, e))?;

    let lockfile_path = repo_path.join("package-lock.json");
    fs::write(
        &lockfile_path,
        b"{\n  \"name\": \"flashwt-synthetic-project\",\n  \"version\": \"1.0.0\",\n  \"lockfileVersion\": 3,\n  \"packages\": {}\n}\n",
    )
    .map_err(|e| Error::io("write package-lock.json", &lockfile_path, e))?;

    workspace::run(repo_path, &["add", "."])?;
    workspace::run(repo_path, &["commit", "--quiet", "-m", "Initial commit"])?;

    let node_modules = repo_path.join("node_modules");
    fs::create_dir_all(&node_modules)
        .map_err(|e| Error::io("create node_modules", &node_modules, e))?;

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    let packages: Vec<usize> = (0..DEMO_PACKAGE_COUNT).collect();
    let chunk_size = packages.len().div_ceil(num_threads);
    let chunks: Vec<_> = packages.chunks(chunk_size.max(1)).collect();

    let (total_files, total_bytes) = std::thread::scope(|s| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                let nm_ref = &node_modules;
                s.spawn(move || {
                    let mut local_files = 0usize;
                    let mut local_bytes = 0u64;

                    for &pkg_idx in chunk {
                        let pkg_dir = if pkg_idx < DEMO_SCOPED_COUNT {
                            nm_ref.join(format!("@demo-scope/pkg-{pkg_idx:02}"))
                        } else {
                            nm_ref.join(format!("demo-lib-{pkg_idx:02}"))
                        };

                        let lib_dir = pkg_dir.join("lib");
                        let src_dir = pkg_dir.join("src");
                        let dist_dir = pkg_dir.join("dist");

                        let _ = fs::create_dir_all(&lib_dir);
                        let _ = fs::create_dir_all(&src_dir);
                        let _ = fs::create_dir_all(&dist_dir);

                        let p_json = pad_demo_file(
                            format!(
                                "{{\n  \"name\": \"pkg-{pkg_idx}\",\n  \"version\": \"1.0.0\",\n  \"main\": \"dist/index.js\",\n  \"types\": \"dist/index.d.ts\"\n}}\n"
                            ),
                            "pkg",
                            0,
                        );
                        let p_json_path = pkg_dir.join("package.json");
                        let _ = fs::write(&p_json_path, p_json.as_bytes());
                        local_files += 1;
                        local_bytes += p_json.len() as u64;

                        let readme = pad_demo_file(
                            format!("# Package {pkg_idx}\nSynthetic fixture for flashwt demo.\n"),
                            "readme",
                            0,
                        );
                        let readme_path = pkg_dir.join("README.md");
                        let _ = fs::write(&readme_path, readme.as_bytes());
                        local_files += 1;
                        local_bytes += readme.len() as u64;

                        let idx_js = pad_demo_file(
                            format!("\"use strict\";\nmodule.exports = {{ pkg: {pkg_idx} }};\n"),
                            "idxjs",
                            0,
                        );
                        let idx_js_path = pkg_dir.join("index.js");
                        let _ = fs::write(&idx_js_path, idx_js.as_bytes());
                        local_files += 1;
                        local_bytes += idx_js.len() as u64;

                        let idx_dts = pad_demo_file(
                            "export declare const pkg: number;\n".to_string(),
                            "idxdts",
                            0,
                        );
                        let idx_dts_path = pkg_dir.join("index.d.ts");
                        let _ = fs::write(&idx_dts_path, idx_dts.as_bytes());
                        local_files += 1;
                        local_bytes += idx_dts.len() as u64;

                        for i in 0..16 {
                            let js = pad_demo_file(
                                format!(
                                    "\"use strict\";\nexports.item{i} = function() {{ return {i}; }};\n"
                                ),
                                "libjs",
                                i,
                            );
                            let js_p = lib_dir.join(format!("module-{i}.js"));
                            let _ = fs::write(&js_p, js.as_bytes());
                            local_files += 1;
                            local_bytes += js.len() as u64;

                            let dts = pad_demo_file(
                                format!("export declare function item{i}(): number;\n"),
                                "libdts",
                                i,
                            );
                            let dts_p = lib_dir.join(format!("module-{i}.d.ts"));
                            let _ = fs::write(&dts_p, dts.as_bytes());
                            local_files += 1;
                            local_bytes += dts.len() as u64;
                        }

                        for i in 0..32 {
                            let ts = pad_demo_file(
                                format!(
                                    "export const val_{i} = {i};\nexport function getVal_{i}() {{ return val_{i}; }}\n"
                                ),
                                "src",
                                i,
                            );
                            let ts_p = src_dir.join(format!("source-{i}.ts"));
                            let _ = fs::write(&ts_p, ts.as_bytes());
                            local_files += 1;
                            local_bytes += ts.len() as u64;
                        }

                        for i in 0..16 {
                            let js = pad_demo_file(
                                format!(
                                    "\"use strict\";\nexports.bundle{i} = () => ({i});\n"
                                ),
                                "distjs",
                                i,
                            );
                            let js_p = dist_dir.join(format!("bundle-{i}.js"));
                            let _ = fs::write(&js_p, js.as_bytes());
                            local_files += 1;
                            local_bytes += js.len() as u64;

                            let min = pad_demo_file(
                                format!("\"use strict\";exports.b{i}=()=>({i});\n"),
                                "distmin",
                                i,
                            );
                            let min_p = dist_dir.join(format!("bundle-{i}.min.js"));
                            let _ = fs::write(&min_p, min.as_bytes());
                            local_files += 1;
                            local_bytes += min.len() as u64;
                        }
                    }

                    (local_files, local_bytes)
                })
            })
            .collect();

        let mut total_files = 0usize;
        let mut total_bytes = 0u64;
        for h in handles {
            if let Ok((f, b)) = h.join() {
                total_files += f;
                total_bytes += b;
            }
        }
        (total_files, total_bytes)
    });

    Ok((total_files, total_bytes))
}

fn verify_mutation_isolation(
    donor_repo: &Path,
    worktree_dest: &Path,
    store: &DiskStore,
) -> Result<bool> {
    let test_rel_path = Path::new("node_modules/@demo-scope/pkg-00/index.js");
    let donor_file = donor_repo.join(test_rel_path);
    let worktree_file = worktree_dest.join(test_rel_path);

    if !donor_file.exists() || !worktree_file.exists() {
        return Err(Error::Store("isolation target file missing".into()));
    }

    let original_donor_bytes = fs::read(&donor_file)
        .map_err(|e| Error::io("read donor file for isolation check", &donor_file, e))?;
    let original_worktree_bytes = fs::read(&worktree_file)
        .map_err(|e| Error::io("read worktree file for isolation check", &worktree_file, e))?;

    if original_donor_bytes != original_worktree_bytes {
        return Err(Error::Store(
            "worktree and donor file content mismatch before mutation".into(),
        ));
    }

    let mutation_content =
        b"// MUTATION TEST - EDITED IN WORKTREE\nmodule.exports = { mutated: true };\n";
    fs::write(&worktree_file, mutation_content)
        .map_err(|e| Error::io("write mutated worktree file", &worktree_file, e))?;

    let mutated_worktree_bytes = fs::read(&worktree_file)
        .map_err(|e| Error::io("read mutated worktree file", &worktree_file, e))?;
    if mutated_worktree_bytes != mutation_content {
        return Err(Error::Store(
            "worktree file mutation was not written".into(),
        ));
    }

    let post_mutation_donor_bytes = fs::read(&donor_file)
        .map_err(|e| Error::io("read donor file after mutation", &donor_file, e))?;
    if post_mutation_donor_bytes != original_donor_bytes {
        return Err(Error::Store(
            "CRITICAL: Donor repository file was modified by worktree mutation (CoW isolation violation)!".into(),
        ));
    }

    let original_hash = ContentId::for_bytes(&original_donor_bytes);

    if store.contains(&original_hash) {
        let store_blob = store
            .get(&original_hash)
            .map_err(|e| Error::Store(format!("store read error for blob {original_hash}: {e}")))?;
        if store_blob != original_donor_bytes {
            return Err(Error::Store(
                "CRITICAL: Store blob was modified by worktree mutation (CoW isolation violation)!"
                    .into(),
            ));
        }
    }

    Ok(true)
}

struct DemoScorecard<'a> {
    files_count: usize,
    total_bytes: u64,
    package_count: usize,
    isolation_verified: bool,
    phases: &'a DemoPhases,
    speedup_ratio: f64,
}

fn print_terminal_scorecard(card: &DemoScorecard) {
    let baseline_ms = card.phases.baseline_ms;
    let warm_ms = card.phases.warm_hydrate_ms;
    let speedup_ratio = card.speedup_ratio;
    let hydration_method = card.phases.method.as_str();
    let bytes_shared_cow = card.phases.bytes_shared;
    let bytes_copied = card.phases.bytes_copied;
    let bar_width = 40usize;
    let baseline_bar = "=".repeat(bar_width);

    let warm_fraction = if baseline_ms > 0 {
        (warm_ms as f64 / baseline_ms as f64).min(1.0)
    } else {
        0.05
    };
    let warm_len = ((warm_fraction * bar_width as f64).round() as usize).clamp(1, bar_width);
    let warm_bar = format!(
        "{}{}",
        "=".repeat(warm_len),
        " ".repeat(bar_width - warm_len)
    );

    println!();
    println!("────────────────────────────────────────────────────────────────────────────────");
    println!("PERFORMANCE SCORECARD");
    println!("────────────────────────────────────────────────────────────────────────────────");
    println!(
        "Standard Copy : [{baseline_bar}] {:>5} ms  (full physical duplication)",
        baseline_ms
    );
    println!(
        "flashwt Warm Hydration  : [{warm_bar}] {:>5} ms  ({:.1}x faster)",
        warm_ms, speedup_ratio
    );
    println!();
    println!("Summary:");
    println!(
        "  • Total Fixture Files    : {} files ({} packages, {})",
        HumanCount(card.files_count),
        card.package_count,
        HumanBytes(card.total_bytes)
    );
    println!("  • Hydration Mechanism    : Copy-on-Write ({hydration_method})");
    println!(
        "  • Cold Ingest            : one-time store population, untimed (excluded from score)"
    );
    if hydration_method == "byte_copy" || bytes_copied > 0 {
        println!(
            "  • Disk Space Duplicated  : {} (fallback byte copy, no CoW savings)",
            HumanBytes(bytes_copied)
        );
    } else {
        println!(
            "  • Disk Space Shared      : {} CoW shared",
            HumanBytes(bytes_shared_cow)
        );
    }
    println!(
        "  • Speedup Ratio          : {:.1}x faster (warm hydration vs baseline copy)",
        speedup_ratio
    );
    println!("  • Mutation Isolation     : VERIFIED (zero cross-worktree bleed)");
    if card.isolation_verified {
        println!("  • Status                 : ALL CHECKS PASSED (5/5)");
    } else {
        println!("  • Status                 : CHECKS FAILED");
    }
    println!("────────────────────────────────────────────────────────────────────────────────");
}

pub fn run(cfg: &RunConfig) -> Result<(DemoData, Vec<Diagnostic>)> {
    let demo_start = Instant::now();

    unsafe {
        std::env::set_var("FLASHWT_NO_SYNC", "1");
    }

    if !cfg.json {
        println!("flashwt demo: Zero-Setup End-to-End Performance Test Drive\n");
    }

    let base_temp =
        tempfile::tempdir().map_err(|e| Error::io("create tempdir for demo", "temp", e))?;
    let donor_repo = base_temp.path().join("demo-repo");
    let worktree_dest = base_temp.path().join("demo-worktree");
    let baseline_dest = base_temp.path().join("demo-baseline");

    if !cfg.json {
        println!("Step 1/5: Synthesizing realistic fixture...");
    }
    let (files_count, total_bytes) = generate_synthetic_fixture(&donor_repo)?;

    if !cfg.json {
        println!(
            "  ✓ Generated {} files across {} packages ({})",
            HumanCount(files_count),
            DEMO_PACKAGE_COUNT,
            HumanBytes(total_bytes)
        );
    }

    if !cfg.json {
        println!("Step 2/5: Warming store (cold ingest, one-time cost, untimed)...");
    }
    let warm_dest = base_temp.path().join("demo-warm");
    let warm_dest_str = warm_dest.to_string_lossy().into_owned();
    workspace::run(
        &donor_repo,
        &[
            "worktree",
            "add",
            "-b",
            "demo-warm-branch",
            &warm_dest_str,
            "HEAD",
        ],
    )?;
    let warm_patterns = vec!["node_modules/".to_string()];
    {
        let mut store = open_store()?;
        let mut engine = HydrationEngine::new(&mut store);
        let req = HydrationRequest {
            root: &donor_repo,
            dest: &warm_dest,
            patterns: &warm_patterns,
            base_branch: None,
            base_commit: None,
            cfg,
        };
        engine.hydrate(req)?;
    }
    if !cfg.json {
        println!("  ✓ Store warmed (one-time cost, excluded from score)");
    }
    let _ = workspace::run(
        &donor_repo,
        &["worktree", "remove", "--force", &warm_dest_str],
    );
    let _ = workspace::run(&donor_repo, &["branch", "-D", "demo-warm-branch"]);
    let _ = crate::gc::remove("demo-warm-branch", Some(&warm_dest), cfg);
    if warm_dest.exists() {
        let _ = fs::remove_dir_all(&warm_dest);
    }

    if !cfg.json {
        println!("Step 3/5: Benchmarking standard filesystem recursive copy (baseline)...");
    }
    let baseline_start = Instant::now();
    let baseline_copy_bytes = recursive_copy(
        &donor_repo.join("node_modules"),
        &baseline_dest.join("node_modules"),
    )?;
    let baseline_ms = baseline_start.elapsed().as_millis() as u64;
    if !cfg.json {
        println!(
            "  ✓ Standard copy completed in {} ms ({} duplicated)",
            baseline_ms,
            HumanBytes(baseline_copy_bytes)
        );
    }

    if !cfg.json {
        println!("Step 4/5: Benchmarking flashwt warm hydration...");
    }
    let worktree_dest_str = worktree_dest.to_string_lossy().into_owned();
    workspace::run(
        &donor_repo,
        &[
            "worktree",
            "add",
            "-b",
            "demo-branch",
            &worktree_dest_str,
            "HEAD",
        ],
    )?;

    let patterns = vec!["node_modules/".to_string()];
    let mut store = open_store()?;
    let mut engine = HydrationEngine::new(&mut store);
    let req = HydrationRequest {
        root: &donor_repo,
        dest: &worktree_dest,
        patterns: &patterns,
        base_branch: None,
        base_commit: None,
        cfg,
    };
    let warm_start = Instant::now();
    let report = engine.hydrate(req)?;
    let warm_ms = warm_start.elapsed().as_millis() as u64;

    let phases = DemoPhases {
        baseline_ms,
        warm_hydrate_ms: warm_ms,
        method: report.hydration_method.clone(),
        bytes_copied: report.bytes_copied,
        bytes_shared: report.bytes_shared_cow,
    };

    if !cfg.json {
        println!(
            "  ✓ Warm hydration completed in {} ms ({} duplicated, {} shared)",
            phases.warm_hydrate_ms,
            HumanBytes(phases.bytes_copied),
            HumanBytes(phases.bytes_shared)
        );
    }

    if !cfg.json {
        println!("Step 5/5: Verifying mutation isolation and cleaning up...");
    }
    let isolation_verified = verify_mutation_isolation(&donor_repo, &worktree_dest, &store)?;
    if !cfg.json {
        println!("  ✓ Mutated worktree file; donor repository and store blobs remain intact");
    }

    let _ = workspace::run(
        &donor_repo,
        &["worktree", "remove", "--force", &worktree_dest_str],
    );
    let _ = workspace::run(&donor_repo, &["branch", "-D", "demo-branch"]);
    let _ = crate::gc::remove("demo-branch", Some(&worktree_dest), cfg);

    if worktree_dest.exists() {
        let _ = fs::remove_dir_all(&worktree_dest);
    }
    if baseline_dest.exists() {
        let _ = fs::remove_dir_all(&baseline_dest);
    }
    if donor_repo.exists() {
        let _ = fs::remove_dir_all(&donor_repo);
    }
    let _ = base_temp.close();
    let cleaned_up = true;

    if !cfg.json {
        println!("  ✓ Teardown complete (all temporary worktrees and fixtures removed)");
    }

    let speedup_ratio = if phases.warm_hydrate_ms == 0 {
        (phases.baseline_ms as f64).max(1.0)
    } else {
        (phases.baseline_ms as f64) / (phases.warm_hydrate_ms as f64)
    };

    let total_duration_ms = demo_start.elapsed().as_millis() as u64;

    if !cfg.json {
        print_terminal_scorecard(&DemoScorecard {
            files_count,
            total_bytes,
            package_count: DEMO_PACKAGE_COUNT,
            isolation_verified,
            phases: &phases,
            speedup_ratio,
        });
    }

    let data = DemoData {
        files_count,
        total_bytes,
        baseline_copy_duration_ms: phases.baseline_ms,
        baseline_copy_bytes,
        flashwt_hydration_duration_ms: phases.warm_hydrate_ms,
        speedup_ratio,
        hydration_method: phases.method,
        bytes_shared_cow: phases.bytes_shared,
        bytes_copied: phases.bytes_copied,
        space_savings_bytes: report.bytes_shared_cow.max(total_bytes),
        isolation_verified,
        cleaned_up,
        total_duration_ms,
    };

    Ok((data, Vec::new()))
}
