use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub trait FileMaterialize: Send + Sync {
    fn name(&self) -> &'static str;

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()>;

    fn shares_inode_with_source(&self) -> bool {
        false
    }
}

#[derive(Debug, Default)]
pub struct HardlinkOut;

impl FileMaterialize for HardlinkOut {
    fn name(&self) -> &'static str {
        "hardlink"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        fs::hard_link(src, dest)?;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(perms.mode() & !0o222);
        fs::set_permissions(dest, perms)
    }

    fn shares_inode_with_source(&self) -> bool {
        true
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
pub struct CloneOut;

#[cfg(target_os = "macos")]
impl FileMaterialize for CloneOut {
    fn name(&self) -> &'static str {
        "copy-on-write"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;

        let blob = fs::File::open(src)?;
        let dir = fs::File::open(dest.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent")
        })?)?;
        let name = dest.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "destination has no name")
        })?;
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL byte"))?;

        let rc = unsafe { libc::fclonefileat(blob.as_raw_fd(), dir.as_raw_fd(), name.as_ptr(), 0) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct ReflinkOut;

#[cfg(target_os = "linux")]
impl FileMaterialize for ReflinkOut {
    fn name(&self) -> &'static str {
        "reflink"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        crate::reflink::reflink_file(src, dest)
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct CopyFileRangeOut;

#[cfg(target_os = "linux")]
impl FileMaterialize for CopyFileRangeOut {
    fn name(&self) -> &'static str {
        "copy_file_range"
    }

    fn materialize_file(&self, src: &Path, dest: &Path) -> io::Result<()> {
        crate::copy_file_range::copy_file_range_file(src, dest)
    }
}

#[cfg(unix)]
pub fn placement_refused(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(code)
            if code == libc::EPERM
                || code == libc::EXDEV
                || code == libc::EMLINK
                || code == libc::ENOTSUP
                || code == libc::EOPNOTSUPP
                || code == libc::ENOSYS
    )
}

#[cfg(not(unix))]
pub fn placement_refused(_e: &io::Error) -> bool {
    true
}

use crate::sys::buffered_copy_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StrategyPolicy {
    #[default]
    Default,

    Hardlink,

    ForceByteCopy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementOutcome {
    pub strategy: &'static str,

    pub is_shared_cow: bool,

    pub is_mode_repaired: bool,

    pub refusal_reason: Option<String>,
}

pub struct Materializer {
    backend: Option<Box<dyn FileMaterialize>>,
    backend_name: &'static str,
    strategy: &'static str,
    refusal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    pub src: PathBuf,

    pub dest: PathBuf,

    pub mode: Option<u32>,

    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReceipt {
    pub total: usize,

    pub placed: usize,

    pub shared_cow: usize,

    pub repaired: usize,

    pub bytes_shared: u64,

    pub bytes_copied: u64,

    pub backend_name: &'static str,

    pub refusal_reason: Option<String>,
}

impl Default for BatchReceipt {
    fn default() -> Self {
        Self {
            total: 0,
            placed: 0,
            shared_cow: 0,
            repaired: 0,
            bytes_shared: 0,
            bytes_copied: 0,
            backend_name: "byte-copy",
            refusal_reason: None,
        }
    }
}

impl Materializer {
    pub fn for_paths(policy: StrategyPolicy, src_root: &Path, dest_root: &Path) -> Self {
        let is_cross = crate::sys::is_cross_device(src_root, dest_root);
        let (reflink_capable, is_ext4) = crate::sys::probe_fs_capabilities(dest_root);
        let refusal_reason = if is_cross {
            let src_fs = crate::sys::filesystem_name(src_root);
            let dest_fs = crate::sys::filesystem_name(dest_root);
            Some(format!(
                "cross-device mount between {} ({src_fs}) and {} ({dest_fs})",
                src_root.display(),
                dest_root.display()
            ))
        } else {
            None
        };
        Self::select_internal(policy, is_cross, reflink_capable, is_ext4, refusal_reason)
    }

    pub fn select(
        policy: StrategyPolicy,
        is_cross_device: bool,
        reflink_capable: bool,
        is_ext4: bool,
    ) -> Self {
        Self::select_internal(policy, is_cross_device, reflink_capable, is_ext4, None)
    }

    fn select_internal(
        policy: StrategyPolicy,
        is_cross_device: bool,
        reflink_capable: bool,
        is_ext4: bool,
        explicit_refusal: Option<String>,
    ) -> Self {
        let (backend, backend_name, strategy, refusal_reason): (
            Option<Box<dyn FileMaterialize>>,
            &'static str,
            &'static str,
            Option<String>,
        ) = match policy {
            StrategyPolicy::ForceByteCopy => (
                None,
                "byte-copy",
                "byte-copy",
                explicit_refusal
                    .or_else(|| Some("forced byte copy policy (ForceByteCopy)".to_string())),
            ),
            StrategyPolicy::Hardlink => (Some(Box::new(HardlinkOut)), "hardlink", "hardlink", None),
            StrategyPolicy::Default => {
                #[cfg(target_os = "macos")]
                {
                    let _ = is_ext4;
                    if !is_cross_device && reflink_capable {
                        (
                            Some(Box::new(CloneOut)),
                            "copy-on-write",
                            "copy-on-write",
                            None,
                        )
                    } else if is_cross_device {
                        (
                            None,
                            "byte-copy",
                            "copy-on-write",
                            explicit_refusal.or_else(|| {
                                Some(
                                    "cross-device mount between source and destination".to_string(),
                                )
                            }),
                        )
                    } else {
                        (
                            None,
                            "byte-copy",
                            "copy-on-write",
                            explicit_refusal.or_else(|| {
                                Some("filesystem does not support APFS clonefile".to_string())
                            }),
                        )
                    }
                }

                #[cfg(target_os = "linux")]
                {
                    if !is_cross_device && reflink_capable {
                        (Some(Box::new(ReflinkOut)), "reflink", "reflink", None)
                    } else if !is_cross_device && is_ext4 {
                        (
                            Some(Box::new(CopyFileRangeOut)),
                            "copy_file_range",
                            "copy_file_range",
                            None,
                        )
                    } else if is_cross_device {
                        (
                            None,
                            "byte-copy",
                            "copy-on-write",
                            explicit_refusal.or_else(|| {
                                Some(
                                    "cross-device mount between source and destination".to_string(),
                                )
                            }),
                        )
                    } else {
                        (
                            None,
                            "byte-copy",
                            "copy-on-write",
                            explicit_refusal.or_else(|| {
                                Some(
                                    "filesystem does not support FICLONE reflink or copy_file_range"
                                        .to_string(),
                                )
                            }),
                        )
                    }
                }

                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    let _ = (is_cross_device, reflink_capable, is_ext4);
                    (
                        None,
                        "byte-copy",
                        "copy-on-write",
                        explicit_refusal.or_else(|| {
                            Some("platform does not support copy-on-write acceleration".to_string())
                        }),
                    )
                }
            }
        };

        Self {
            backend,
            backend_name,
            strategy,
            refusal_reason,
        }
    }

    pub fn strategy(&self) -> &'static str {
        self.strategy
    }

    pub fn backend(&self) -> Option<&dyn FileMaterialize> {
        self.backend.as_deref()
    }

    pub fn selected_backend(&self) -> &'static str {
        self.backend_name
    }

    pub fn refusal_reason(&self) -> Option<&str> {
        self.refusal_reason.as_deref()
    }

    pub fn materialize_file(
        &self,
        src: &Path,
        dest: &Path,
        mode: Option<u32>,
    ) -> io::Result<PlacementOutcome> {
        let (placed, refusal_reason) = match self.place_once(src, dest) {
            Ok(placed) => placed,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                    self.place_once(src, dest)?
                } else {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        };

        let mut is_mode_repaired = false;
        if let Some(target_mode) = mode {
            let shared_inode = placed
                && self
                    .backend
                    .as_deref()
                    .is_some_and(|b| b.shares_inode_with_source());
            is_mode_repaired = self.finalize_mode(shared_inode, target_mode, src, dest)?;
        }

        let is_shared_cow = placed && !is_mode_repaired;

        Ok(PlacementOutcome {
            strategy: self.strategy,
            is_shared_cow,
            is_mode_repaired,
            refusal_reason,
        })
    }

    pub fn materialize_batch(&self, items: &[BatchItem]) -> io::Result<BatchReceipt> {
        if items.is_empty() {
            return Ok(BatchReceipt {
                backend_name: self.backend_name,
                ..Default::default()
            });
        }
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let num_workers = num_cpus.clamp(4, 8).min(items.len()).max(1);

        let next_idx = AtomicUsize::new(0);
        let placed = AtomicUsize::new(0);
        let shared = AtomicUsize::new(0);
        let repaired = AtomicUsize::new(0);
        let bytes_shared = AtomicU64::new(0);
        let bytes_copied = AtomicU64::new(0);
        let refusal_slot: Mutex<Option<String>> = Mutex::new(None);
        let err_slot: Mutex<Option<io::Error>> = Mutex::new(None);

        std::thread::scope(|s| {
            for _ in 0..num_workers {
                s.spawn(|| {
                    let mut local_placed = 0usize;
                    let mut local_shared = 0usize;
                    let mut local_repaired = 0usize;
                    let mut local_bytes_shared = 0u64;
                    let mut local_bytes_copied = 0u64;
                    loop {
                        if err_slot.lock().unwrap_or_else(|p| p.into_inner()).is_some() {
                            break;
                        }
                        let idx = next_idx.fetch_add(1, Ordering::Relaxed);
                        if idx >= items.len() {
                            break;
                        }
                        let item = &items[idx];
                        let outcome = match self.materialize_file(&item.src, &item.dest, item.mode)
                        {
                            Ok(outcome) => outcome,
                            Err(e) => {
                                let mut slot = err_slot.lock().unwrap_or_else(|p| p.into_inner());
                                if slot.is_none() {
                                    *slot = Some(e);
                                }
                                break;
                            }
                        };
                        local_placed += 1;
                        if outcome.is_shared_cow {
                            local_shared += 1;
                            local_bytes_shared += item.size;
                        } else {
                            local_bytes_copied += item.size;
                            if let Some(refusal) = outcome.refusal_reason {
                                let mut slot =
                                    refusal_slot.lock().unwrap_or_else(|p| p.into_inner());
                                if slot.is_none() {
                                    *slot = Some(refusal);
                                }
                            }
                        }
                        if outcome.is_mode_repaired {
                            local_repaired += 1;
                        }
                    }
                    placed.fetch_add(local_placed, Ordering::Relaxed);
                    shared.fetch_add(local_shared, Ordering::Relaxed);
                    repaired.fetch_add(local_repaired, Ordering::Relaxed);
                    bytes_shared.fetch_add(local_bytes_shared, Ordering::Relaxed);
                    bytes_copied.fetch_add(local_bytes_copied, Ordering::Relaxed);
                });
            }
        });

        if let Some(err) = err_slot.into_inner().unwrap_or_default() {
            return Err(err);
        }

        let placed_count = placed.into_inner();
        let shared_count = shared.into_inner();
        let final_refusal = refusal_slot.into_inner().unwrap_or_default().or_else(|| {
            if shared_count < placed_count {
                self.refusal_reason.clone()
            } else {
                None
            }
        });

        Ok(BatchReceipt {
            total: items.len(),
            placed: placed_count,
            shared_cow: shared_count,
            repaired: repaired.into_inner(),
            bytes_shared: bytes_shared.into_inner(),
            bytes_copied: bytes_copied.into_inner(),
            backend_name: self.backend_name,
            refusal_reason: final_refusal,
        })
    }

    fn place_once(&self, src: &Path, dest: &Path) -> io::Result<(bool, Option<String>)> {
        if dest.exists() || dest.is_symlink() {
            let _ = fs::remove_file(dest);
        }
        if let Some(backend) = &self.backend {
            match backend.materialize_file(src, dest) {
                Ok(()) => return Ok((true, None)),
                Err(e) if placement_refused(&e) => {
                    let reason = if let Some(code) = e.raw_os_error() {
                        crate::sys::refusal_reason_for_errno(code)
                    } else {
                        format!("placement refused: {e}")
                    };
                    buffered_copy_file(src, dest)?;
                    return Ok((false, Some(reason)));
                }
                Err(e) => return Err(e),
            }
        }
        buffered_copy_file(src, dest)?;
        Ok((false, self.refusal_reason.clone()))
    }

    fn finalize_mode(
        &self,
        shared_inode: bool,
        target_mode: u32,
        src: &Path,
        dest: &Path,
    ) -> io::Result<bool> {
        let current = fs::metadata(dest)?.permissions().mode();
        if shared_inode {
            if current & 0o111 == target_mode & 0o111 {
                return Ok(false);
            }
            fs::remove_file(dest)?;
            buffered_copy_file(src, dest)?;
            fs::set_permissions(dest, fs::Permissions::from_mode(target_mode))?;
            return Ok(true);
        }
        if current & 0o7777 != target_mode & 0o7777 {
            fs::set_permissions(dest, fs::Permissions::from_mode(target_mode))?;
        }
        Ok(false)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;
