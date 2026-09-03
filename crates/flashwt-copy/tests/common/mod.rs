use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct TreeFixture {
    pub src: PathBuf,

    #[allow(dead_code)]
    pub outside: PathBuf,
    _base: tempfile::TempDir,
}

impl TreeFixture {
    pub fn heavy_tree(files: usize) -> TreeFixture {
        let base = tempfile::tempdir().expect("tempdir");
        let src = base.path().join("src");
        let outside = base.path().join("outside");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(outside.join("outside.txt"), "never followed\n").expect("write outside");

        for i in 0..files {
            let dir = src.join(format!("pkg{:02}/nested", i % 20));
            fs::create_dir_all(&dir).expect("mkdir pkg");
            fs::write(
                dir.join(format!("file-{i}.txt")),
                format!("file {i} of {files}\n"),
            )
            .expect("write file");
        }

        fs::create_dir_all(src.join("scripts")).expect("mkdir scripts");
        let script = src.join("scripts/exec.sh");
        fs::write(&script, "#!/bin/sh\necho hi\n").expect("write script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");

        unix_symlink("../outside/outside.txt", &src.join("link-to-outside.txt"));

        TreeFixture {
            src,
            outside,
            _base: base,
        }
    }
}

pub fn unix_symlink(target: &str, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("symlink");
    #[cfg(not(unix))]
    unreachable!("copy backends are unix-only; asked to symlink {target} at {link:?}");
}

pub fn list_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).expect("read_dir") {
            let p = entry.expect("dir entry").path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn assert_trees_identical(src: &Path, dest: &Path) {
    let src_files = list_files(src);
    let dest_files = list_files(dest);

    let rel = |root: &Path, p: &Path| p.strip_prefix(root).expect("strip prefix").to_path_buf();
    let src_rel: Vec<PathBuf> = src_files.iter().map(|p| rel(src, p)).collect();
    let dest_rel: Vec<PathBuf> = dest_files.iter().map(|p| rel(dest, p)).collect();
    assert_eq!(src_rel, dest_rel, "tree shape differs");

    for (s, d) in src_files.iter().zip(dest_files.iter()) {
        if s.is_symlink() {
            continue;
        }
        let want = fs::read(s).expect("read src file");
        let got = fs::read(d).expect("read dest file");
        assert_eq!(want, got, "content differs for {}", s.display());
    }

    let exec_src = src.join("scripts/exec.sh");
    let mode = |p: &Path| fs::metadata(p).expect("metadata").permissions().mode();
    assert_eq!(mode(&exec_src) & 0o111, 0o111, "exec bit lost in {dest:?}");

    let link = dest.join("link-to-outside.txt");
    let target =
        std::fs::read_link(&link).expect("symlink must be recreated, not materialized as a file");
    assert_eq!(target, std::path::Path::new("../outside/outside.txt"));
}
