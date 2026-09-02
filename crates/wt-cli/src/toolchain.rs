//! Toolchain relocation and cache exclusion logic (ticket 08).
//!
//! Post-hydration sanitization for virtual environments (.venv)
//! and exclusion of volatile compiler incremental caches.

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Determines whether a repo-relative path matches a volatile host compiler cache
/// that should be excluded from starter manifests and hydration.
///
/// Volatile caches include:
/// - Rust incremental compiler caches (`target/debug/incremental/`, `target/**/incremental/`)
/// - Next.js build cache (`.next/cache/`)
/// - Vite dependency cache (`node_modules/.vite/`)
pub fn is_volatile_cache(rel_path: &str) -> bool {
    crate::hydration_filter::is_volatile_cache(rel_path)
}

/// Recursively find all virtual environment root directories under `dir`.
fn find_venvs(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut visited = HashSet::new();
    find_venvs_inner(dir, out, &mut visited);
}

fn find_venvs_inner(dir: &Path, out: &mut Vec<PathBuf>, visited: &mut HashSet<(u64, u64)>) {
    let Ok(meta) = fs::metadata(dir) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let key = (meta.dev(), meta.ino());
    if !visited.insert(key) {
        return;
    }
    if dir.join("pyvenv.cfg").is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == ".venv") || path.join("pyvenv.cfg").is_file() {
            out.push(path);
        } else if path
            .file_name()
            .is_some_and(|n| n != "node_modules" && n != "target" && n != ".git")
        {
            find_venvs_inner(&path, out, visited);
        }
    }
}

/// Run post-hydration sanitization across all hydrated directories.
///
/// For Python virtual environments (.venv), rewrites absolute paths in `pyvenv.cfg`,
/// updates `bin/activate*` scripts with the new target worktree path, and patches
/// shebang lines in `bin/*` script executables. Preserves `.pyc` bytecode files without rewriting.
pub fn relocate_toolchains(
    src_root: &Path,
    dest_root: &Path,
    hydrated_dirs: &[PathBuf],
) -> Result<()> {
    for rel in hydrated_dirs {
        let dest_dir = dest_root.join(rel);
        if !dest_dir.exists() {
            continue;
        }
        let mut venvs = Vec::new();
        if dest_dir.join("pyvenv.cfg").is_file() || rel.file_name().is_some_and(|n| n == ".venv") {
            venvs.push(dest_dir.clone());
        } else if dest_dir.is_dir() {
            find_venvs(&dest_dir, &mut venvs);
        }

        for venv in venvs {
            relocate_venv(src_root, dest_root, &venv)?;
        }
    }
    Ok(())
}

fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut result = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(needle) {
            result.extend_from_slice(replacement);
            i += needle.len();
        } else {
            result.push(haystack[i]);
            i += 1;
        }
    }
    result
}

/// Sanitize and relocate a single Python virtual environment directory.
pub fn relocate_venv(src_root: &Path, dest_root: &Path, venv_dir: &Path) -> Result<()> {
    let mut replacements: Vec<(String, String)> = Vec::new();

    let mut add_pair = |s: &str, d: &str| {
        if !s.is_empty()
            && !d.is_empty()
            && s != d
            && !replacements.iter().any(|(from, _)| from == s)
        {
            replacements.push((s.to_string(), d.to_string()));
        }
    };

    let src_str = src_root.to_string_lossy().into_owned();
    let dest_str = dest_root.to_string_lossy().into_owned();
    add_pair(&src_str, &dest_str);

    if let (Ok(sc), Ok(dc)) = (fs::canonicalize(src_root), fs::canonicalize(dest_root)) {
        let sc_str = sc.to_string_lossy().into_owned();
        let dc_str = dc.to_string_lossy().into_owned();
        add_pair(&sc_str, &dc_str);

        if let Some(stripped_s) = sc_str.strip_prefix("/private") {
            if let Some(stripped_d) = dc_str.strip_prefix("/private") {
                add_pair(stripped_s, stripped_d);
            }
        }
    }
    if let Some(stripped_s) = src_str.strip_prefix("/private") {
        if let Some(stripped_d) = dest_str.strip_prefix("/private") {
            add_pair(stripped_s, stripped_d);
        }
    }

    let replace_paths = |text: &str| -> String {
        let mut result = text.to_string();
        for (from, to) in &replacements {
            result = result.replace(from, to);
        }
        result
    };

    // 1. pyvenv.cfg
    let cfg_path = venv_dir.join("pyvenv.cfg");
    if cfg_path.is_file() {
        if let Ok(content) = fs::read_to_string(&cfg_path) {
            let updated = replace_paths(&content);
            if updated != content {
                let meta = fs::metadata(&cfg_path).ok();
                fs::write(&cfg_path, updated.as_bytes())
                    .map_err(|e| Error::io("update pyvenv.cfg", &cfg_path, e))?;
                if let Some(m) = meta {
                    let _ = fs::set_permissions(&cfg_path, m.permissions());
                }
            }
        }
    }

    // 2. bin/ (and Scripts/ on Windows-like layouts)
    let bin_dirs = [venv_dir.join("bin"), venv_dir.join("Scripts")];
    for bin_dir in &bin_dirs {
        if !bin_dir.is_dir() {
            continue;
        }
        let entries = match fs::read_dir(bin_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();

            // .pyc bytecode files are left untouched for self-healing
            if file_name.ends_with(".pyc") {
                continue;
            }

            let Ok(file_type) = entry.file_type() else {
                continue;
            };

            // Symlinks in bin/
            if file_type.is_symlink() {
                if let Ok(target) = fs::read_link(&path) {
                    let target_str = target.to_string_lossy().into_owned();
                    let updated_target = replace_paths(&target_str);
                    if updated_target != target_str {
                        let _ = fs::remove_file(&path);
                        let _ = std::os::unix::fs::symlink(Path::new(&updated_target), &path);
                    }
                }
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };

            // bin/activate* shell scripts
            let is_activate =
                file_name.starts_with("activate") || file_name.starts_with("Activate");
            if is_activate {
                if let Ok(content) = fs::read_to_string(&path) {
                    let updated = replace_paths(&content);
                    if updated != content {
                        fs::write(&path, updated.as_bytes())
                            .map_err(|e| Error::io("update activate script", &path, e))?;
                        let _ = fs::set_permissions(&path, meta.permissions());
                    }
                }
                continue;
            }

            // Executables in bin/*: check for shebang lines via byte buffer
            if let Ok(bytes) = fs::read(&path) {
                if !bytes.starts_with(b"#!") {
                    continue;
                }
                let newline = bytes.iter().position(|&b| b == b'\n');
                let header_end = newline.map(|p| p + 1).unwrap_or(bytes.len());
                let (header, rest) = bytes.split_at(header_end);
                let mut new_header = header.to_vec();
                for (from, to) in &replacements {
                    new_header = replace_bytes(&new_header, from.as_bytes(), to.as_bytes());
                }
                if new_header != header {
                    let mut new_bytes = Vec::with_capacity(new_header.len() + rest.len());
                    new_bytes.extend_from_slice(&new_header);
                    new_bytes.extend_from_slice(rest);
                    fs::write(&path, &new_bytes)
                        .map_err(|e| Error::io("update shebang script", &path, e))?;
                    let _ = fs::set_permissions(&path, meta.permissions());
                }
            }
        }
    }

    // 3. .pth files in lib/python*/site-packages
    for lib_name in ["lib", "lib64"] {
        let lib_dir = venv_dir.join(lib_name);
        if !lib_dir.is_dir() {
            continue;
        }
        let Ok(lib_entries) = fs::read_dir(&lib_dir) else {
            continue;
        };
        for lib_entry in lib_entries.flatten() {
            let lib_path = lib_entry.path();
            if !lib_path.is_dir() {
                continue;
            }
            let name = lib_entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("python") {
                continue;
            }
            let site_packages = lib_path.join("site-packages");
            if !site_packages.is_dir() {
                continue;
            }
            let Ok(sp_entries) = fs::read_dir(&site_packages) else {
                continue;
            };
            for sp_entry in sp_entries.flatten() {
                let p = sp_entry.path();
                let fname = sp_entry.file_name().to_string_lossy().into_owned();
                if !fname.ends_with(".pth") {
                    continue;
                }
                let Ok(ft) = sp_entry.file_type() else {
                    continue;
                };
                if !ft.is_file() {
                    continue;
                }
                if let Ok(bytes) = fs::read(&p) {
                    let mut new_bytes = bytes.clone();
                    for (from, to) in &replacements {
                        new_bytes = replace_bytes(&new_bytes, from.as_bytes(), to.as_bytes());
                    }
                    if new_bytes != bytes {
                        let meta = fs::metadata(&p).ok();
                        fs::write(&p, &new_bytes)
                            .map_err(|e| Error::io("update .pth file", &p, e))?;
                        if let Some(m) = meta {
                            let _ = fs::set_permissions(&p, m.permissions());
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
    fn test_relocate_venv_rewrites_cfg_activate_and_shebangs() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src_repo");
        let dest = temp.path().join("dest_repo");

        let src_venv = src.join(".venv");
        let dest_venv = dest.join(".venv");
        fs::create_dir_all(dest_venv.join("bin")).unwrap();

        // 1. pyvenv.cfg
        let cfg_content = format!(
            "home = /usr/bin\ncommand = /usr/bin/python3 -m venv {}\ninclude-system-site-packages = false\n",
            src_venv.display()
        );
        fs::write(dest_venv.join("pyvenv.cfg"), cfg_content).unwrap();

        // 2. activate script
        let activate_content = format!(
            r#"# This file must be used with "source bin/activate" *from bash*
VIRTUAL_ENV="{}"
export VIRTUAL_ENV
_OLD_VIRTUAL_PATH="$PATH"
PATH="$VIRTUAL_ENV/bin:$PATH"
export PATH
"#,
            src_venv.display()
        );
        fs::write(dest_venv.join("bin/activate"), activate_content).unwrap();

        // 3. Script with shebang
        let script_content = format!(
            "#!{}/bin/python3\nimport sys\nprint('hello')\n",
            src_venv.display()
        );
        let script_path = dest_venv.join("bin/pytest");
        fs::write(&script_path, script_content).unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        // 4. .pyc file in bin
        let pyc_path = dest_venv.join("bin/test.pyc");
        let original_pyc = b"\x6f\r\r\n\0\0\0\0dummybytecode";
        fs::write(&pyc_path, original_pyc).unwrap();

        // Run relocation
        relocate_venv(&src, &dest, &dest_venv).unwrap();

        // Verify pyvenv.cfg
        let updated_cfg = fs::read_to_string(dest_venv.join("pyvenv.cfg")).unwrap();
        assert!(updated_cfg.contains(&format!(
            "command = /usr/bin/python3 -m venv {}",
            dest_venv.display()
        )));
        assert!(!updated_cfg.contains(&src_venv.to_string_lossy().into_owned()));

        // Verify activate
        let updated_act = fs::read_to_string(dest_venv.join("bin/activate")).unwrap();
        assert!(updated_act.contains(&format!("VIRTUAL_ENV=\"{}\"", dest_venv.display())));
        assert!(!updated_act.contains(&src_venv.to_string_lossy().into_owned()));

        // Verify script shebang
        let updated_script = fs::read_to_string(&script_path).unwrap();
        assert!(updated_script.starts_with(&format!("#!{}/bin/python3\n", dest_venv.display())));
        let perms = fs::metadata(&script_path).unwrap().permissions().mode();
        assert_eq!(perms & 0o777, 0o755);

        // Verify .pyc preserved
        let pyc_bytes = fs::read(&pyc_path).unwrap();
        assert_eq!(pyc_bytes, original_pyc);
    }

    #[test]
    fn test_relocate_venv_rewrites_pth_files() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src_repo");
        let dest = temp.path().join("dest_repo");
        let dest_venv = dest.join(".venv");
        fs::create_dir_all(dest_venv.join("lib/python3.11/site-packages")).unwrap();
        fs::create_dir_all(dest_venv.join("bin")).unwrap();
        fs::write(dest_venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();

        let pth_content = format!("{}\n{}/src\n", src.display(), src.display());
        let pth_path = dest_venv.join("lib/python3.11/site-packages/foo.pth");
        fs::write(&pth_path, &pth_content).unwrap();
        let editable_path =
            dest_venv.join("lib/python3.11/site-packages/_editable_foo.pth");
        let editable_content = format!("{}\n", src.display());
        fs::write(&editable_path, &editable_content).unwrap();

        relocate_venv(&src, &dest, &dest_venv).unwrap();

        let updated = fs::read_to_string(&pth_path).unwrap();
        assert!(updated.contains(&dest.display().to_string()));
        assert!(!updated.contains(&src.display().to_string()));
        let updated_editable = fs::read_to_string(&editable_path).unwrap();
        assert!(updated_editable.contains(&dest.display().to_string()));
        assert!(!updated_editable.contains(&src.display().to_string()));
    }

    #[test]
    fn test_relocate_venv_rewrites_binary_shebang_non_utf8() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src_repo");
        let dest = temp.path().join("dest_repo");
        let src_venv = src.join(".venv");
        let dest_venv = dest.join(".venv");
        fs::create_dir_all(dest_venv.join("bin")).unwrap();
        fs::write(dest_venv.join("pyvenv.cfg"), "home = /usr/bin\n").unwrap();

        let bin_path = dest_venv.join("bin/binary_launcher");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(format!("#!{}/bin/python\n", src_venv.display()).as_bytes());
        bytes.extend_from_slice(&[0xFF, 0xFE, 0xFD]);
        bytes.extend_from_slice(b"\nbinary payload");
        let original_rest = bytes[bytes.iter().position(|&b| b == b'\n').unwrap() + 1..].to_vec();
        fs::write(&bin_path, &bytes).unwrap();
        fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o755)).unwrap();

        relocate_venv(&src, &dest, &dest_venv).unwrap();

        let updated = fs::read(&bin_path).unwrap();
        assert!(updated.starts_with(format!("#!{}/bin/python\n", dest_venv.display()).as_bytes()));
        // payload after newline preserved, including non-utf8 bytes
        let payload_start = updated.iter().position(|&b| b == b'\n').unwrap() + 1;
        assert_eq!(&updated[payload_start..], original_rest.as_slice());
        let perms = fs::metadata(&bin_path).unwrap().permissions().mode();
        assert_eq!(perms & 0o777, 0o755);
        // ensure src path not present
        assert!(!updated.windows(src_venv.to_string_lossy().len()).any(|w| w == src_venv.to_string_lossy().as_bytes()));
    }

    #[test]
    fn test_find_venvs_cycle_detection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cycle_root");
        fs::create_dir_all(root.join("a/b")).unwrap();
        // create a venv at a/b
        fs::write(root.join("a/b/pyvenv.cfg"), "home = /usr/bin\n").unwrap();
        // recursive symlink a/b/cycle -> ../a
        std::os::unix::fs::symlink(root.join("a"), root.join("a/b/cycle")).unwrap();

        let mut venvs = Vec::new();
        find_venvs(&root, &mut venvs);
        // Should find the venv without panicking / infinite recursion
        assert_eq!(venvs.len(), 1);
        assert!(venvs[0].ends_with("a/b"));
    }
}
