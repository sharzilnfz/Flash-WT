//! Hardlink backend with copy-on-shared-write protection (ticket 07).
//!
//! Plain hardlinks share one inode between trees: an in-place rewrite
//! in one tree silently corrupts every other tree holding the same
//! file (the pnpm lesson, spec user story 5). This backend keeps the
//! speed of shared inodes but removes the hazard: after linking, the
//! write bits are stripped from the shared inode (execute permission
//! is preserved), so an in-place rewrite fails loudly instead of
//! poisoning siblings. Writers that replace the file — unlink plus
//! recreate, or rename-over — break the share and get a private,
//! writable copy. That is the copy-on-shared-write behavior package
//! managers already exercise trees for.
//!
//! The trade-off is inherent to hardlinks: permissions live on the
//! inode, so the source path loses its write bits too. Selection
//! therefore hands this backend immutable sources only: it is picked
//! under [`crate::SourcePolicy::Immutable`] and skipped entirely
//! under [`crate::SourcePolicy::Any`]. In practice that means
//! content-addressed store objects and snapshot trees.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::copy_tree::copy_tree;
use crate::{BackendKind, CopyBackend, Error, Result, Safety};

#[derive(Debug, Default)]
pub struct HardlinkBackend;

impl CopyBackend for HardlinkBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Hardlink
    }

    /// Safe as of ticket 07: shared inodes are made read-only, so an
    /// in-place rewrite cannot reach sibling trees or the source.
    fn safety(&self) -> Safety {
        Safety::Safe
    }

    /// Hardlinks work on almost every POSIX filesystem, within one
    /// filesystem. Known exceptions (FAT/exFAT family, common network
    /// filesystems) are rejected here so selection falls through to
    /// deep copy instead of failing mid-copy.
    fn supports(&self, dir: &Path) -> bool {
        fs_supports_hardlinks(dir)
    }

    fn copy_dir(&self, src: &Path, dest: &Path) -> Result<()> {
        crate::copy_tree::staged_copy(dest, self.safety(), &mut |staging| {
            let mut link_file = |from: &Path, to: &Path| {
                fs::hard_link(from, to)?;
                // Strip write bits from the shared inode; keep exec so
                // scripts and shims stay runnable.
                let mut perms = fs::metadata(to)?.permissions();
                perms.set_mode(perms.mode() & !0o222);
                fs::set_permissions(to, perms)
            };
            copy_tree(src, staging, &mut link_file).map_err(Error::Io)
        })
    }
}

#[cfg(target_os = "macos")]
fn fs_supports_hardlinks(dir: &Path) -> bool {
    use std::ffi::CStr;

    // A pure predicate over the shared statfs probe: reject
    // read-only mounts and filesystem families without hardlinks.
    let Ok(st) = crate::sys::statfs_of(dir) else {
        return false;
    };
    if st.f_flags & libc::MNT_RDONLY as u32 != 0 {
        return false;
    }
    let fstype = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
    let name = fstype.to_string_lossy().to_lowercase();
    !matches!(
        name.as_str(),
        "msdos" | "vfat" | "exfat" | "smbfs" | "cifs" | "webdav" | "afpfs"
    )
}

#[cfg(target_os = "linux")]
fn fs_supports_hardlinks(dir: &Path) -> bool {
    const MSDOS_SUPER_MAGIC: i64 = 0x4d44;
    const CIFS_MAGIC_NUMBER: i64 = 0xff53_4d42;
    const SMB_SUPER_MAGIC: i64 = 0x517b;

    let Ok(st) = crate::sys::statfs_of(dir) else {
        return false;
    };
    // Linux `statfs` exposes no flags field; `statvfs::f_flag` carries
    // the read-only bit. A failed statvfs probe is ignored, matching
    // the original behavior.
    if matches!(crate::sys::read_only(dir), Ok(true)) {
        return false;
    }
    st.f_type != MSDOS_SUPER_MAGIC && st.f_type != CIFS_MAGIC_NUMBER && st.f_type != SMB_SUPER_MAGIC
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn fs_supports_hardlinks(_dir: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::PathBuf;

    /// A small tree with one executable script, mirroring the shapes
    /// real heavy directories hold.
    fn fixture(base: &Path, tag: &str) -> PathBuf {
        let src = base.join(tag);
        fs::create_dir_all(src.join("a/b")).expect("mkdir");
        fs::write(src.join("a/b/f.txt"), format!("{tag} bytes\n")).expect("write");
        fs::write(src.join("a/b/run.sh"), "#!/bin/sh\necho hi\n").expect("write script");
        fs::set_permissions(src.join("a/b/run.sh"), fs::Permissions::from_mode(0o755))
            .expect("chmod");
        src
    }

    #[test]
    fn copies_tree_and_strips_write_bits_keeping_exec() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = fixture(dir.path(), "src");
        let dest = dir.path().join("dest");

        HardlinkBackend.copy_dir(&src, &dest).expect("copy_dir");
        assert_eq!(
            fs::read_to_string(dest.join("a/b/f.txt")).expect("read"),
            "src bytes\n"
        );

        for file in ["a/b/f.txt", "a/b/run.sh"] {
            let mode = fs::metadata(dest.join(file))
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o222, 0, "{file} must not be writable");
        }
        let script = fs::metadata(dest.join("a/b/run.sh"))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(script & 0o111, 0o111, "exec bit must survive");

        assert!(matches!(
            HardlinkBackend.copy_dir(&src, &dest),
            Err(Error::DestinationExists)
        ));
    }

    #[test]
    fn linked_files_share_one_readonly_inode_with_the_source() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = fixture(dir.path(), "src");
        let dest_a = dir.path().join("tree-a");
        let dest_b = dir.path().join("tree-b");

        HardlinkBackend.copy_dir(&src, &dest_a).expect("copy a");
        HardlinkBackend.copy_dir(&src, &dest_b).expect("copy b");

        let original = fs::read_to_string(src.join("a/b/f.txt")).expect("read");
        for tree in [&src, &dest_a, &dest_b] {
            let meta = fs::metadata(tree.join("a/b/f.txt")).expect("meta");
            assert_eq!(meta.nlink(), 3, "three paths must share one inode");
            assert!(
                meta.permissions().readonly(),
                "shared inode must be read-only"
            );
        }

        // The pnpm hazard, neutralized: an in-place rewrite anywhere
        // fails loudly instead of poisoning the other two paths.
        for tree in [&src, &dest_a, &dest_b] {
            let Err(err) = fs::write(tree.join("a/b/f.txt"), "poison\n") else {
                panic!("in-place rewrite of a shared inode must fail");
            };
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }
        assert_eq!(
            fs::read_to_string(dest_b.join("a/b/f.txt")).expect("read"),
            original,
            "sibling content changed"
        );
    }

    #[test]
    fn replacement_writes_break_the_share_and_get_a_private_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src = fixture(dir.path(), "src");
        let dest_a = dir.path().join("tree-a");
        let dest_b = dir.path().join("tree-b");
        HardlinkBackend.copy_dir(&src, &dest_a).expect("copy a");
        HardlinkBackend.copy_dir(&src, &dest_b).expect("copy b");

        // Package-manager pattern: write a temp file, rename over the
        // old path. This must succeed and stay private to tree-a.
        let tmp = dest_a.join("a/b/f.txt.tmp");
        fs::write(&tmp, "rewritten by package manager\n").expect("write tmp");
        fs::rename(&tmp, dest_a.join("a/b/f.txt")).expect("rename-over");

        assert_eq!(
            fs::read_to_string(dest_a.join("a/b/f.txt")).expect("read"),
            "rewritten by package manager\n"
        );
        assert_eq!(
            fs::read_to_string(dest_b.join("a/b/f.txt")).expect("read"),
            "src bytes\n",
            "rename-over leaked into a sibling tree"
        );
        // And the replacement is fully writable: the share is broken.
        fs::write(dest_a.join("a/b/f.txt"), "second rewrite\n").expect("private rewrite");
    }

    #[test]
    fn reports_unsupported_for_missing_paths() {
        assert!(!HardlinkBackend.supports(Path::new("/definitely/not/here")));
    }
}
