use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::verified::VerifiedLedger;
use crate::{ContentId, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsCapabilities {
    pub device_id: u64,

    pub fs_type: u64,

    pub reflink_capable: bool,
}

impl FsCapabilities {
    pub fn is_ext4(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.fs_type == (libc::EXT4_SUPER_MAGIC as u64)
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreDiskUsage {
    pub objects_bytes: u64,

    pub snapshots_bytes: u64,

    pub mirrors_bytes: u64,

    pub refs_bytes: u64,

    pub caches_bytes: u64,

    pub total_bytes: u64,
}

pub fn probe_fs(path: &Path) -> io::Result<FsCapabilities> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;

        let meta = fs::metadata(path)?;
        let device_id = meta.dev();

        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c_path.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }

        #[cfg(target_os = "linux")]
        let (fs_type, reflink_capable) = {
            let f_type = st.f_type as libc::c_long;
            let is_reflink = f_type == (libc::BTRFS_SUPER_MAGIC as libc::c_long)
                || f_type == (libc::XFS_SUPER_MAGIC as libc::c_long);
            (f_type as u64, is_reflink)
        };

        #[cfg(target_os = "macos")]
        let (fs_type, reflink_capable) = {
            use std::ffi::CStr;
            let fstype = unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) };
            let is_apfs = fstype.to_string_lossy().eq_ignore_ascii_case("apfs");
            (st.f_type as u64, is_apfs)
        };

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let (fs_type, reflink_capable) = (st.f_type as u64, false);

        Ok(FsCapabilities {
            device_id,
            fs_type,
            reflink_capable,
        })
    }
    #[cfg(not(unix))]
    {
        Ok(FsCapabilities {
            device_id: 0,
            fs_type: 0,
            reflink_capable: false,
        })
    }
}

pub struct DiskStore {
    root: PathBuf,

    ledger: Mutex<VerifiedLedger>,
    fs_caps: FsCapabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    pub examined: u64,

    pub reclaimed: u64,
}

const BATCH_FILE_LIMIT: usize = 256;

enum BatchItemSource<'a> {
    Bytes(&'a [u8]),
    File(&'a Path),
}

struct BatchItem<'a> {
    id: ContentId,
    source: BatchItemSource<'a>,
}

impl DiskStore {
    pub fn open(root: impl Into<PathBuf>) -> io::Result<DiskStore> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("refs"))?;
        let ledger = Mutex::new(VerifiedLedger::open(&root));
        let fs_caps = probe_fs(&root)?;
        Ok(DiskStore {
            root,
            ledger,
            fs_caps,
        })
    }

    pub fn fs_capabilities(&self) -> FsCapabilities {
        self.fs_caps
    }

    fn ledger(&self) -> MutexGuard<'_, VerifiedLedger> {
        self.ledger.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn lock_refs(&self) -> io::Result<crate::fsutil::FlockGuard> {
        let dir = fs::File::open(self.root.join("refs"))?;
        crate::fsutil::FlockGuard::lock_exclusive(dir)
    }

    pub fn flush(&self) -> io::Result<()> {
        self.ledger().save_if_dirty()
    }

    pub fn ensure_verified(&self, id: &ContentId) -> Result<()> {
        let path = self.object_path(id);
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownContent(*id));
            }
            Err(e) => return Err(e.into()),
        };
        let size = meta.len();
        let mtime = meta.modified()?;
        if self.ledger().matches(id, size, mtime) {
            return Ok(());
        }
        self.verify_digest(id)?;

        self.ledger()
            .record(*id, crate::verified::Fingerprint { size, mtime });
        Ok(())
    }

    pub fn verify_digest(&self, id: &ContentId) -> Result<()> {
        Self::verify_file(&self.object_path(id), id)
    }

    pub(crate) fn verify_file(path: &Path, expected: &ContentId) -> Result<()> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(Error::UnknownContent(*expected));
            }
            Err(e) => return Err(e.into()),
        };
        let mut reader = io::BufReader::with_capacity(1024 * 1024, file);
        let mut hasher = Sha256::new();
        io::copy(&mut reader, &mut hasher)?;
        if ContentId(hasher.finalize().into()) != *expected {
            return Err(Error::Corrupted(*expected));
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn object_path(&self, id: &ContentId) -> PathBuf {
        let hex = id.to_string();
        self.root.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    pub fn blob_path(&self, id: &ContentId) -> PathBuf {
        self.object_path(id)
    }

    pub(crate) fn ref_path(&self, id: &ContentId) -> PathBuf {
        self.root.join("refs").join(id.to_string())
    }

    pub(crate) fn read_ref_count(&self, id: &ContentId) -> Result<u64> {
        let text = match fs::read_to_string(self.ref_path(id)) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(Error::Io(e)),
        };
        text.trim()
            .parse::<u64>()
            .map_err(|e| Error::Io(io::Error::other(format!("bad ref count file: {e}"))))
    }

    fn write_ref_count(&self, id: &ContentId, count: u64) -> Result<()> {
        let dir = self.root.join("refs");
        let path = dir.join(id.to_string());
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        writeln!(tmp, "{count}")?;

        tmp.as_file().sync_all()?;
        tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
        crate::fsutil::sync_parent_dir(&path)?;
        Ok(())
    }

    pub fn ids(&self) -> Result<Vec<ContentId>> {
        let mut out = Vec::new();
        let shards = fs::read_dir(self.root.join("objects")).map_err(Error::Io)?;
        for shard in shards.flatten() {
            let Ok(ft) = shard.file_type() else {
                continue;
            };
            if !ft.is_dir() {
                continue;
            }
            let blobs = match fs::read_dir(shard.path()) {
                Ok(b) => b,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(Error::Io(e)),
            };
            for blob in blobs.flatten() {
                let hex = format!(
                    "{}{}",
                    shard.file_name().to_string_lossy(),
                    blob.file_name().to_string_lossy()
                );
                if let Some(id) = ContentId::from_hex(&hex) {
                    out.push(id);
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    pub fn delete(&mut self, id: &ContentId) -> Result<()> {
        match fs::remove_file(self.ref_path(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        match fs::remove_file(self.object_path(id)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        if let Some(shard) = self.object_path(id).parent() {
            let _ = fs::remove_dir(shard);
        }
        self.ledger().forget(id);
        Ok(())
    }

    pub fn compact_snapshot_journal(&self) -> io::Result<()> {
        crate::snapindex::compact_journal(self.root())
    }

    pub fn disk_usage(&self) -> Result<StoreDiskUsage> {
        Self::inspect_disk_usage(self.root())
    }

    pub fn inspect_disk_usage(root: &Path) -> Result<StoreDiskUsage> {
        if !root.exists() {
            return Ok(StoreDiskUsage::default());
        }
        let mut usage = StoreDiskUsage::default();
        let entries = match fs::read_dir(root) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(usage),
            Err(e) => return Err(Error::Io(e)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            match name_str.as_ref() {
                "objects" => {
                    usage.objects_bytes = crate::fsutil::measure_tree_size(&path);
                }
                "snapshots" => {
                    usage.snapshots_bytes = crate::fsutil::measure_tree_size(&path);
                }
                "worktrees" => {
                    usage.mirrors_bytes = crate::fsutil::measure_tree_size(&path);
                }
                "refs" => {
                    usage.refs_bytes = crate::fsutil::measure_tree_size(&path);
                }
                "ingest-cache.tsv" | "verified.tsv" | "lru.tsv" | "index.tsv" | "journal.tsv" => {
                    usage.caches_bytes += crate::fsutil::measure_tree_size(&path);
                }
                _ => {
                    usage.caches_bytes += crate::fsutil::measure_tree_size(&path);
                }
            }
        }
        usage.total_bytes = usage.objects_bytes
            + usage.snapshots_bytes
            + usage.mirrors_bytes
            + usage.refs_bytes
            + usage.caches_bytes;
        Ok(usage)
    }

    pub fn sweep(&mut self, max_age: Duration) -> Result<Swept> {
        self.sweep_ext(max_age, false).map(|(s, _)| s)
    }

    pub fn sweep_ext(&mut self, max_age: Duration, dry_run: bool) -> Result<(Swept, u64)> {
        let _ = crate::snapindex::compact_journal(self.root());
        let cutoff = SystemTime::now()
            .checked_sub(max_age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let ids = self.ids()?;
        let examined = ids.len() as u64;
        let mut reclaimed = 0u64;
        let mut reclaimed_bytes = 0u64;
        for id in ids {
            match fs::metadata(self.ref_path(&id)) {
                Ok(_) => {
                    if self.read_ref_count(&id)? > 0 {
                        continue;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let meta = match fs::metadata(self.object_path(&id)) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(Error::Io(e)),
            };
            let modified = match meta.modified() {
                Ok(m) => m,
                Err(e) => return Err(Error::Io(e)),
            };
            if modified > cutoff {
                continue;
            }
            if !dry_run {
                self.delete(&id)?;
            }
            reclaimed += 1;
            reclaimed_bytes += meta.len();
        }
        Ok((
            Swept {
                examined,
                reclaimed,
            },
            reclaimed_bytes,
        ))
    }

    pub fn link_out(&self, id: &ContentId, dest: &Path) -> Result<()> {
        self.verify_digest(id)?;
        fs::hard_link(self.object_path(id), dest)?;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(perms.mode() & !0o222);
        Ok(fs::set_permissions(dest, perms)?)
    }

    pub fn put(&mut self, content: &[u8]) -> Result<ContentId> {
        let ids = self.put_batch(&[content])?;
        Ok(ids[0])
    }

    pub fn put_batch(&mut self, contents: &[&[u8]]) -> Result<Vec<ContentId>> {
        if contents.is_empty() {
            return Ok(Vec::new());
        }
        let items: Vec<BatchItem<'_>> = contents
            .iter()
            .map(|&b| BatchItem {
                id: ContentId(Sha256::digest(b).into()),
                source: BatchItemSource::Bytes(b),
            })
            .collect();
        let mut dummy_buf = [];
        self.put_batch_internal(&items, &mut dummy_buf)
    }

    pub fn put_file_with_id(&mut self, src_path: &Path, id: &ContentId) -> Result<ContentId> {
        let mut buf = vec![0u8; 64 * 1024];
        self.put_file_with_id_buf(src_path, id, &mut buf)
    }

    pub fn put_file_with_id_buf(
        &mut self,
        src_path: &Path,
        id: &ContentId,
        buf: &mut [u8],
    ) -> Result<ContentId> {
        let ids = self.put_files_batch_buf(&[(src_path, *id)], buf)?;
        Ok(ids[0])
    }

    pub fn put_files_batch<P: AsRef<Path>>(
        &mut self,
        files: &[(P, ContentId)],
    ) -> Result<Vec<ContentId>> {
        let mut buf = vec![0u8; 64 * 1024];
        self.put_files_batch_buf(files, &mut buf)
    }

    pub fn put_files_batch_buf<P: AsRef<Path>>(
        &mut self,
        files: &[(P, ContentId)],
        buf: &mut [u8],
    ) -> Result<Vec<ContentId>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let items: Vec<BatchItem<'_>> = files
            .iter()
            .map(|(p, id)| BatchItem {
                id: *id,
                source: BatchItemSource::File(p.as_ref()),
            })
            .collect();
        let mut local_buf;
        let copy_buf = if buf.is_empty() {
            local_buf = vec![0u8; 64 * 1024];
            &mut local_buf[..]
        } else {
            buf
        };
        self.put_batch_internal(&items, copy_buf)
    }

    pub fn put_file(&mut self, src_path: &Path) -> Result<ContentId> {
        let mut buf = vec![0u8; 64 * 1024];
        let id = crate::ingest::stream_hash_file_with_buf(src_path, &mut buf)?;
        self.put_file_with_id_buf(src_path, &id, &mut buf)
    }

    fn put_batch_internal(
        &mut self,
        items: &[BatchItem<'_>],
        buf: &mut [u8],
    ) -> Result<Vec<ContentId>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut result_ids = Vec::with_capacity(items.len());
        let mut staged_ids = std::collections::BTreeSet::new();
        let mut to_stage = Vec::new();

        for item in items {
            result_ids.push(item.id);
            if self.contains(&item.id) || !staged_ids.insert(item.id) {
                continue;
            }
            to_stage.push((item.id, self.object_path(&item.id), &item.source));
        }

        if to_stage.is_empty() {
            return Ok(result_ids);
        }

        let sync_disabled = crate::fsutil::is_sync_disabled();
        let mut touched_shard_dirs = std::collections::BTreeSet::new();
        let mut persisted_blobs = Vec::with_capacity(to_stage.len());
        let mut created_any_new_shard = false;

        for chunk in to_stage.chunks(BATCH_FILE_LIMIT) {
            let mut temp_files = Vec::with_capacity(chunk.len());
            for (id, path, source) in chunk {
                let hex = id.to_string();
                let shard_dir = self.root.join("objects").join(&hex[..2]);
                if !shard_dir.exists() {
                    fs::create_dir_all(&shard_dir)?;
                    created_any_new_shard = true;
                }
                let mut tmp = tempfile::NamedTempFile::new_in(&shard_dir)?;
                match source {
                    BatchItemSource::Bytes(bytes) => {
                        tmp.write_all(bytes)?;
                    }
                    BatchItemSource::File(src_path) => {
                        let mut src = fs::File::open(src_path)?;
                        loop {
                            let n = src.read(buf)?;
                            if n == 0 {
                                break;
                            }
                            tmp.write_all(&buf[..n])?;
                        }
                    }
                }
                temp_files.push((*id, path.clone(), shard_dir, tmp));
            }

            if !sync_disabled {
                for (_, _, _, tmp) in &temp_files {
                    tmp.as_file().sync_all()?;
                }
            }

            for (id, path, shard_dir, tmp) in temp_files {
                tmp.persist(&path).map_err(|e| Error::Io(e.error))?;
                touched_shard_dirs.insert(shard_dir);
                persisted_blobs.push((id, path));
            }
        }

        if !sync_disabled {
            for shard_dir in &touched_shard_dirs {
                crate::fsutil::sync_dir(shard_dir)?;
            }
            if created_any_new_shard {
                let objects_dir = self.root.join("objects");
                crate::fsutil::sync_dir(&objects_dir)?;
            }
        }

        let perms = fs::Permissions::from_mode(0o644);
        for (id, path) in &persisted_blobs {
            let _ = fs::set_permissions(path, perms.clone());
            if let Ok(meta) = fs::metadata(path) {
                if let Ok(mtime) = meta.modified() {
                    self.ledger().record(
                        *id,
                        crate::verified::Fingerprint {
                            size: meta.len(),
                            mtime,
                        },
                    );
                }
            }
        }

        Ok(result_ids)
    }

    pub fn get(&self, id: &ContentId) -> Result<Vec<u8>> {
        let path = self.object_path(id);
        if !path.exists() {
            return Err(Error::UnknownContent(*id));
        }
        let content = fs::read(&path)?;
        if ContentId(Sha256::digest(&content).into()) != *id {
            return Err(Error::Corrupted(*id));
        }
        Ok(content)
    }

    pub fn contains(&self, id: &ContentId) -> bool {
        self.object_path(id).exists()
    }

    pub fn add_ref(&mut self, id: &ContentId) -> Result<()> {
        let _lock = self.lock_refs()?;

        let current = match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !self.contains(id) {
                    return Err(Error::UnknownContent(*id));
                }
                0
            }
            Err(e) => return Err(e.into()),
        };

        let next = current.checked_add(1).ok_or_else(|| {
            Error::Io(io::Error::other(format!(
                "reference count overflow for {id}"
            )))
        })?;
        self.write_ref_count(id, next)
    }

    pub fn release_ref(&mut self, id: &ContentId) -> Result<()> {
        let _lock = self.lock_refs()?;
        if !self.contains(id) {
            return Err(Error::RefCountUnderflow(*id));
        }
        let current = match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id)?,
            Err(_) => 0,
        };
        if current == 0 {
            return Err(Error::RefCountUnderflow(*id));
        }
        self.write_ref_count(id, current - 1)
    }

    pub fn ref_count(&self, id: &ContentId) -> Result<u64> {
        if !self.contains(id) {
            return Err(Error::UnknownContent(*id));
        }
        match fs::metadata(self.ref_path(id)) {
            Ok(_) => self.read_ref_count(id),
            Err(_) => Ok(0),
        }
    }
}

impl Drop for DiskStore {
    fn drop(&mut self) {
        let _ = self.ledger().save_if_dirty();
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_filesystem_and_caches_capabilities_on_store_open() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = DiskStore::open(temp.path()).expect("open store");
        let caps = store.fs_capabilities();

        #[cfg(unix)]
        assert!(caps.device_id > 0, "device id must be non-zero on unix");

        #[cfg(target_os = "linux")]
        {
            assert!(
                caps.fs_type > 0,
                "filesystem magic must be reported on linux"
            );
        }

        let direct_caps = probe_fs(temp.path()).expect("probe_fs");
        assert_eq!(caps, direct_caps);
    }
}
