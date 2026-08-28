//! Lease persistence for ephemeral scratch and isolate worktrees (ticket 03).
//!
//! A lease record is written to `<store>/worktrees/scratch-<id>.lease`
//! when an ephemeral worktree is spawned. It persists:
//! - Canonical worktree path
//! - Canonical git directory path
//! - Process identifier (PID)
//! - Process start time fingerprint (to detect PID reuse across reboots/kills)
//! - Expiration timestamp (seconds since UNIX epoch)
//!
//! When `--run` finishes cleanly, the sandbox and lease are removed immediately.
//! If the process is terminated abruptly (SIGKILL/reboot), `wt sweep` (ticket 04)
//! reads `.lease` files to reclaim dead or expired sandboxes.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::mirror::{escape, unescape};

/// Default lease time-to-live if unspecified: 1 hour (3600 seconds).
pub const DEFAULT_LEASE_TTL_SECS: u64 = 3600;

/// Parsed lease record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLease {
    /// Identifier (e.g. "scratch-abc12345" or "abc12345").
    pub id: String,
    /// Canonical path of the worktree directory.
    pub worktree: PathBuf,
    /// Canonical path of the worktree's git directory.
    pub gitdir: PathBuf,
    /// Owning process ID.
    pub pid: u32,
    /// Process start time fingerprint.
    pub start_time: u64,
    /// Expiration timestamp in seconds since UNIX epoch.
    pub expires_at: u64,
}

impl WorktreeLease {
    /// Construct a new in-memory lease record.
    pub fn new(
        id: impl Into<String>,
        worktree: PathBuf,
        gitdir: PathBuf,
        pid: u32,
        start_time: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            id: id.into(),
            worktree,
            gitdir,
            pid,
            start_time,
            expires_at,
        }
    }

    /// Render the lease record as TSV.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str("v1\tlease\t");
        out.push_str(&escape(&self.worktree.to_string_lossy()));
        out.push('\t');
        out.push_str(&escape(&self.gitdir.to_string_lossy()));
        out.push('\t');
        out.push_str(&self.pid.to_string());
        out.push('\t');
        out.push_str(&self.start_time.to_string());
        out.push('\t');
        out.push_str(&self.expires_at.to_string());
        out.push('\n');
        out
    }

    /// Parse a lease record from TSV text.
    pub fn parse(id: &str, text: &str) -> Result<Self, String> {
        let line = text.lines().next().ok_or("empty lease file")?;
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 7 || fields[0] != "v1" || fields[1] != "lease" {
            return Err(format!("bad v1 lease header: {line:?}"));
        }
        let worktree = PathBuf::from(unescape(fields[2])?);
        if worktree.as_os_str().is_empty() {
            return Err("empty worktree path in lease".into());
        }
        let gitdir = PathBuf::from(unescape(fields[3])?);
        if gitdir.as_os_str().is_empty() {
            return Err("empty gitdir path in lease".into());
        }
        let pid = fields[4]
            .parse::<u32>()
            .map_err(|e| format!("bad pid: {e}"))?;
        let start_time = fields[5]
            .parse::<u64>()
            .map_err(|e| format!("bad start_time: {e}"))?;
        let expires_at = fields[6]
            .parse::<u64>()
            .map_err(|e| format!("bad expires_at: {e}"))?;

        Ok(Self {
            id: id.to_string(),
            worktree,
            gitdir,
            pid,
            start_time,
            expires_at,
        })
    }
}

/// Compute lease file path `<root>/worktrees/scratch-<id>.lease`.
pub fn lease_path(root: &Path, id: &str) -> PathBuf {
    let filename = if id.starts_with("scratch-") {
        format!("{id}.lease")
    } else {
        format!("scratch-{id}.lease")
    };
    root.join("worktrees").join(filename)
}

/// Publish a lease record atomically and crash-durably.
pub fn publish(root: &Path, lease: &WorktreeLease) -> io::Result<PathBuf> {
    let dir = root.join("worktrees");
    fs::create_dir_all(dir.join("tmp"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir.join("tmp"))?;
    tmp.write_all(lease.serialize().as_bytes())?;
    let final_path = lease_path(root, &lease.id);
    crate::fsutil::durable_write_then_rename(tmp.path(), &final_path)?;
    Ok(final_path)
}

/// Remove a lease record from disk if present.
pub fn remove(root: &Path, id: &str) -> io::Result<bool> {
    let path = lease_path(root, id);
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// One lease file found on disk.
#[derive(Debug)]
pub struct ReadLease {
    /// Where the lease file lives.
    pub path: PathBuf,
    /// Identifier extracted from the file name.
    pub id: String,
    /// Modification time of the lease file.
    pub modified: SystemTime,
    /// Parsed lease record or parse error reason.
    pub lease: Result<WorktreeLease, String>,
}

/// Read all `<root>/worktrees/*.lease` files.
pub fn read_all(root: &Path) -> Vec<ReadLease> {
    let mut out = Vec::new();
    let dir = root.join("worktrees");
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(std::ffi::OsStr::new("lease")) {
            continue;
        }
        let Some(id) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        let (modified, text) = match (fs::metadata(&path), fs::read_to_string(&path)) {
            (Ok(meta), Ok(text)) => (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), text),
            _ => continue,
        };
        let lease = WorktreeLease::parse(&id, &text);
        out.push(ReadLease {
            path,
            id,
            modified,
            lease,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Query process start time fingerprint for `pid`.
pub fn process_start_time(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let stat_path = format!("/proc/{pid}/stat");
        if let Ok(content) = fs::read_to_string(stat_path) {
            if let Some(idx) = content.rfind(')') {
                let rest = &content[idx + 1..];
                let fields: Vec<&str> = rest.split_whitespace().collect();
                if let Some(start_time_str) = fields.get(19) {
                    if let Ok(st) = start_time_str.parse::<u64>() {
                        return Some(st);
                    }
                }
            }
        }
        let proc_path = format!("/proc/{pid}");
        if let Ok(meta) = fs::metadata(proc_path) {
            if let Ok(mtime) = meta.modified() {
                if let Ok(dur) = mtime.duration_since(SystemTime::UNIX_EPOCH) {
                    return Some(dur.as_secs());
                }
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        use std::mem::MaybeUninit;
        #[repr(C)]
        struct ProcBsdInfo {
            pbi_flags: u32,
            pbi_status: u32,
            pbi_xstatus: u32,
            pbi_pid: u32,
            pbi_ppid: u32,
            pbi_uid: libc::uid_t,
            pbi_gid: libc::gid_t,
            pbi_ruid: libc::uid_t,
            pbi_rgid: libc::gid_t,
            pbi_svuid: libc::uid_t,
            pbi_svgid: libc::gid_t,
            rfu_1: u32,
            pbi_comm: [libc::c_char; 16],
            pbi_name: [libc::c_char; 32],
            pbi_nfiles: u32,
            pbi_pgid: u32,
            pbi_pjobc: u32,
            e_tdev: u32,
            e_tpgid: u32,
            pbi_nice: i32,
            pbi_start_tvsec: u64,
            pbi_start_tvusec: u64,
        }
        extern "C" {
            fn proc_pidinfo(
                pid: libc::c_int,
                flavor: libc::c_int,
                arg: u64,
                buffer: *mut libc::c_void,
                buffersize: libc::c_int,
            ) -> libc::c_int;
        }
        const PROC_PIDTBSDINFO: libc::c_int = 3;
        let mut info = MaybeUninit::<ProcBsdInfo>::uninit();
        let ret = unsafe {
            proc_pidinfo(
                pid as libc::c_int,
                PROC_PIDTBSDINFO,
                0,
                info.as_mut_ptr() as *mut libc::c_void,
                std::mem::size_of::<ProcBsdInfo>() as libc::c_int,
            )
        };
        if ret as usize == std::mem::size_of::<ProcBsdInfo>() {
            let info = unsafe { info.assume_init() };
            return Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec);
        }
        None
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Get current process start time fingerprint.
pub fn current_process_start_time() -> u64 {
    let pid = std::process::id();
    process_start_time(pid).unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    })
}

/// Check whether a process is still alive and has matching start time.
pub fn is_process_alive(pid: u32, expected_start_time: u64) -> bool {
    if pid == 0 {
        return false;
    }
    let res = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if res != 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return false;
        }
    }
    if expected_start_time > 0 {
        if let Some(current_st) = process_start_time(pid) {
            if current_st != expected_start_time {
                return false;
            }
        }
    }
    true
}

/// Check if a lease record is expired.
pub fn is_lease_expired(lease: &WorktreeLease) -> bool {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    lease.expires_at > 0 && now >= lease.expires_at
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_serialization_and_parsing_round_trip() {
        let lease = WorktreeLease::new(
            "scratch-abc12345",
            PathBuf::from("/tmp/wt-worktrees/repo-scratch-abc12345"),
            PathBuf::from("/tmp/repo/.git/worktrees/scratch-abc12345"),
            12345,
            67890,
            1800000000,
        );
        let serialized = lease.serialize();
        assert!(serialized.starts_with("v1\tlease\t"));
        let parsed = WorktreeLease::parse("scratch-abc12345", &serialized).expect("parse lease");
        assert_eq!(parsed, lease);
    }

    #[test]
    fn lease_serialization_round_trips_weird_paths() {
        let lease = WorktreeLease::new(
            "scratch-weird",
            PathBuf::from("/tmp/wt-worktrees/repo\tscratch\nweird %25"),
            PathBuf::from("/tmp/repo/.git/worktrees/scratch\rweird"),
            9999,
            1111,
            2222,
        );
        let serialized = lease.serialize();
        assert!(!serialized.contains('\r'));
        let parsed = WorktreeLease::parse("scratch-weird", &serialized).expect("parse weird lease");
        assert_eq!(parsed, lease);
    }

    #[test]
    fn lease_publish_read_all_and_remove() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root).expect("create store");

        let lease = WorktreeLease::new(
            "scratch-test123",
            PathBuf::from("/tmp/worktree1"),
            PathBuf::from("/tmp/gitdir1"),
            std::process::id(),
            current_process_start_time(),
            1900000000,
        );

        let path = publish(&store_root, &lease).expect("publish lease");
        assert_eq!(path, lease_path(&store_root, "scratch-test123"));
        assert!(path.exists());

        let leases = read_all(&store_root);
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].lease.as_ref().unwrap(), &lease);

        assert!(remove(&store_root, "scratch-test123").expect("remove lease"));
        assert!(!remove(&store_root, "scratch-test123").expect("remove again is false"));
        assert!(read_all(&store_root).is_empty());
    }

    #[test]
    fn process_liveness_check() {
        let my_pid = std::process::id();
        let my_st = current_process_start_time();
        assert!(is_process_alive(my_pid, my_st));

        // Dead PID
        assert!(!is_process_alive(999_999_999, 0));
    }

    #[test]
    fn lease_expiration_check() {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let active_lease = WorktreeLease::new(
            "active",
            PathBuf::from("/tmp/w1"),
            PathBuf::from("/tmp/g1"),
            1,
            1,
            now + 3600,
        );
        assert!(!is_lease_expired(&active_lease));

        let expired_lease = WorktreeLease::new(
            "expired",
            PathBuf::from("/tmp/w2"),
            PathBuf::from("/tmp/g2"),
            1,
            1,
            now - 10,
        );
        assert!(is_lease_expired(&expired_lease));
    }
}
