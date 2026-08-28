//! macOS bulk directory walking (`getattrlistbulk`).
//!
//! The legacy ingest walk pays one `readdir` per directory plus one
//! `stat` per regular file — two syscalls where the kernel can answer
//! with one. On macOS, `getattrlistbulk(2)` returns directory entries
//! WITH type, size, mtime, and permissions in bulk (hundreds of
//! entries per syscall), which cuts tens of thousands of syscalls off
//! a 40k-file ingest.
//!
//! Everything here degrades gracefully: any failure surfaces as an
//! [`io::Error`] and the caller falls back to the portable
//! `read_dir`+`metadata` walk unchanged.
//!
//! Record layout (from xnu `bsd/vfs/vfs_attrlist.c`; NOTE that
//! contrary to the man page's "descending bit order" description,
//! attributes are packed in the kernel's fixed SOURCE order):
//!
//! ```text
//! u32  record length (includes trailing pad to 8-byte alignment)
//! u32  returned commonattr bitmap      \
//! u32  returned volattr                } attribute_set_t
//! u32  returned dirattr                }
//! u32  returned fileattr               /
//! u32  entry error slot (always present in bulk output, 0 on success,
//!      NOT advertised in the returned bitmap)
//! ...fixed-width attribute data in kernel source order, each rounded
//!    up to a 4-byte multiple...
//! ...variable tail: name bytes...
//! ```
//!
//! For our request the fixed area is exactly:
//! NAME (attrreference {i32 offset from itself, u32 length incl NUL}),
//! OBJTYPE (u32), MODTIME (struct timespec, 2×i64), ACCESSMASK (u32,
//! FULL st_mode including the S_IFMT type bits), then — for
//! non-directories — the file attribute DATALENGTH (u64).

use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Initial reusable buffer capacity per worker thread (32KB).
pub const INITIAL_BUFFER_CAPACITY: usize = 32 * 1024;
/// Maximum dynamic scratch buffer ceiling (1MB).
pub const MAX_BUFFER_CAPACITY: usize = 1024 * 1024;

thread_local! {
    static SCRATCH_BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Run a closure with access to the worker thread's reusable scratch buffer.
/// Lazily allocates a 32KB buffer on first access and reuses it for all
/// subsequent directory reads on the calling thread.
pub fn with_scratch_buffer<R>(f: impl FnOnce(&mut Vec<u8>) -> R) -> R {
    SCRATCH_BUFFER.with(|cell| {
        let mut buf = cell.borrow_mut();
        let target_len = buf.capacity().max(INITIAL_BUFFER_CAPACITY);
        if buf.len() != target_len {
            buf.resize(target_len, 0);
        }
        f(&mut buf)
    })
}

/// One walked filesystem entry, relative to the walk root.
///
/// Mirrors what the legacy walk learns from `DirEntry::file_type()`
/// plus one `fs::metadata()` per regular file — except symlinks are
/// never followed and their targets are left for the caller to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkEntry {
    /// Slash-separated path relative to the walk root. The root
    /// directory itself is NOT among the entries.
    pub rel_path: String,
    /// True for directory entries.
    pub is_dir: bool,
    /// True for symlink entries; targets are never followed.
    pub is_symlink: bool,
    /// True for regular-file entries.
    pub is_file: bool,
    /// Data length in bytes (regular files only; 0 otherwise).
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime_secs: u64,
    /// Modification time, sub-second nanoseconds.
    pub mtime_nanos: u32,
    /// Access mode bits (`st_mode`, including S_IFMT type bits; the
    /// legacy walk's `MetadataExt::mode()` equivalent — mask with
    /// `0o7777` for permissions).
    pub mode: u32,
}

// --- raw getattrlistbulk interface ---
//
// Declared locally rather than through the `libc` crate so the fast
// path does not depend on the crate version having it. Constants match
// <sys/attr.h> and <sys/vnode.h> exactly — several differ from the
// getattrlist(2) prose by more than an order of magnitude, so resist
// any urge to "fix" them from memory.

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const ATTR_BIT_MAP_COUNT: u16 = 5;

// vnode types (`vtype`, <sys/vnode.h>). FIFOs, sockets, and anything
// unrecognized are reported with all kind flags false; the caller
// decides whether that is fatal.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

// Common-attribute bits (<sys/attr.h>).
const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
const ATTR_CMN_ACCESSMASK: u32 = 0x0002_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;

// File-attribute bits (<sys/attr.h>): sizes live here, not under CMN.
const ATTR_FILE_DATALENGTH: u32 = 0x0000_0200;

// Every common attribute we ask for; used to reject surprises.
const REQUEST_COMMON: u32 = ATTR_CMN_NAME
    | ATTR_CMN_OBJTYPE
    | ATTR_CMN_MODTIME
    | ATTR_CMN_ACCESSMASK
    | ATTR_CMN_RETURNED_ATTRS;
const REQUEST_FILE: u32 = ATTR_FILE_DATALENGTH;

#[cfg(target_os = "macos")]
const O_DIRECTORY: i32 = 0x0010_0000;
#[cfg(target_os = "macos")]
const O_CLOEXEC: i32 = 0x0100_0000;

#[repr(C)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

// SAFETY contract for every call below: each function is called with
// pointers derived from valid, NUL-terminated C strings or valid
// buffers for the duration of the call only; none of the three keep
// references after returning. The declarations are repeated locally
// (instead of via the `libc` crate) so the fast path does not depend
// on the crate version carrying them.
#[cfg(target_os = "macos")]
unsafe extern "C" {
    /// SAFETY (callers): `fd` must be an open directory file
    /// descriptor; `attr_list`/`attr_buf` must be valid for
    /// `attr_buf_size` bytes.
    fn getattrlistbulk(
        fd: i32,
        attr_list: *const AttrList,
        attr_buf: *mut core::ffi::c_void,
        attr_buf_size: usize,
        options: u64,
    ) -> i32;
    /// SAFETY (callers): `path` must point to a valid NUL-terminated
    /// C string; variadic mode is required for the O_CREAT-less open
    /// used here (no third argument is passed).
    fn open(path: *const core::ffi::c_char, oflag: i32, ...) -> i32;
    /// SAFETY (callers): `fd` must be an open descriptor and must not
    /// be used again afterwards.
    fn close(fd: i32) -> i32;
}

/// Walk `src` recursively, returning every entry under it (not the
/// root itself) in one bulk syscall per directory. Any failure —
/// including a filesystem that refuses `getattrlistbulk` — comes back
/// as an [`io::Error`] so the caller can fall back to the portable
/// walk.
#[cfg(target_os = "macos")]
pub fn walk(src: &Path) -> io::Result<Vec<BulkEntry>> {
    let mut out = Vec::new();
    // Depth-first over (absolute dir, relative prefix). Output order
    // does not matter: every consumer sorts or keys by path.
    let mut stack = vec![(src.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let c_dir = CString::new(dir.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path has interior NUL"))?;
        let fd = unsafe { open(c_dir.as_ptr(), libc::O_RDONLY | O_DIRECTORY | O_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = bulk_read_dir(fd, &dir, &prefix, &mut out, &mut stack);
        unsafe { close(fd) };
        result?;
    }
    Ok(out)
}

/// Drain one open directory fd into `out`, pushing subdirectories onto
/// `stack`.
#[cfg(target_os = "macos")]
fn bulk_read_dir(
    fd: i32,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<BulkEntry>,
    stack: &mut Vec<(PathBuf, String)>,
) -> io::Result<()> {
    let attr_list = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: REQUEST_COMMON,
        volattr: 0,
        dirattr: 0,
        fileattr: REQUEST_FILE,
        forkattr: 0,
    };
    with_scratch_buffer(|buf| {
        loop {
            let n =
                unsafe { getattrlistbulk(fd, &attr_list, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n == 0 {
                return Ok(()); // end of directory
            }
            if n < 0 {
                let err = io::Error::last_os_error();
                // One entry did not fit the buffer: retry with a bigger one
                // rather than giving up on the whole walk, up to a 1MB
                // ceiling (then degrade to the portable walk).
                if err.raw_os_error() == Some(libc::ERANGE) && buf.len() < MAX_BUFFER_CAPACITY {
                    let new_cap = (buf.len() * 2).min(MAX_BUFFER_CAPACITY);
                    buf.resize(new_cap, 0);
                    continue;
                }
                return Err(err);
            }
            parse_bulk_buffer(buf, n as usize, dir, prefix, out, stack)?;
        }
    })
}

/// Parse strictly `n` records returned in `buf` by `getattrlistbulk`.
///
/// Reads strictly `n` records from `buf` and validates record byte lengths
/// and buffer bounds to prevent ghost entry corruptions.
pub fn parse_bulk_buffer(
    buf: &[u8],
    n: usize,
    dir: &Path,
    prefix: &str,
    out: &mut Vec<BulkEntry>,
    stack: &mut Vec<(PathBuf, String)>,
) -> io::Result<()> {
    let mut offset = 0usize;
    for _ in 0..n {
        if offset + 4 > buf.len() {
            return Err(io::Error::other(
                "getattrlistbulk buffer overrun reading record length",
            ));
        }
        let record_len = read_u32(buf, offset) as usize;
        if record_len < 36 || record_len % 4 != 0 {
            return Err(io::Error::other(
                "getattrlistbulk returned a malformed record length",
            ));
        }
        if offset
            .checked_add(record_len)
            .is_none_or(|end| end > buf.len())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bulk record length exceeds buffer bounds",
            ));
        }
        let record = &buf[offset..offset + record_len];
        offset += record_len;
        parse_record(record, dir, prefix, out, stack)?;
    }
    Ok(())
}

/// Decode one attrbuffer record: 4-byte length, 16-byte
/// `attribute_set_t` of returned bitmaps, then the fixed-width data
/// for exactly the attributes we requested, in the kernel's source
/// order (see module docs), then the name bytes in the variable tail.
pub fn parse_record(
    record: &[u8],
    dir: &Path,
    prefix: &str,
    out: &mut Vec<BulkEntry>,
    stack: &mut Vec<(PathBuf, String)>,
) -> io::Result<()> {
    if record.len() < 36 {
        return Err(io::Error::other("getattrlistbulk record too short"));
    }
    let common = read_u32(record, 4);
    let volattr = read_u32(record, 8);
    let dirattr = read_u32(record, 12);
    let filebits = read_u32(record, 16);

    // Anything beyond what we asked for means the layout assumptions
    // above are void; bail into the portable fallback.
    if common & !REQUEST_COMMON != 0
        || volattr != 0
        || dirattr != 0
        || filebits & !REQUEST_FILE != 0
    {
        return Err(io::Error::other("unexpected attributes in attrbuffer"));
    }
    if common & ATTR_CMN_NAME == 0 || common & ATTR_CMN_OBJTYPE == 0 {
        return Err(io::Error::other("getattrlistbulk returned no name/type"));
    }

    // Reserved entry error slot at @20. Must be 0 on success.
    let entry_error = read_u32(record, 20);
    if entry_error != 0 {
        return Err(io::Error::other("getattrlistbulk entry error"));
    }

    // Fixed-width data, consumed in kernel source order.
    // NAME: attrreference_t { i32 dataoffset (relative to itself),
    // u32 length including the trailing NUL }; bytes live in the tail.
    let mut pos = 24usize;
    let dataoffset = read_i32(record, pos);
    let name_len = read_u32(record, pos + 4) as usize;
    pos += 8;

    if name_len == 0 || dataoffset < 0 {
        return Err(io::Error::other(
            "getattrlistbulk returned a bad name reference",
        ));
    }
    let name_start = match (24isize).checked_add(dataoffset as isize) {
        Some(s) if s >= 24 => s as usize,
        _ => return Err(io::Error::other("getattrlistbulk invalid name offset")),
    };
    if name_start
        .checked_add(name_len)
        .is_none_or(|end| end > record.len())
    {
        return Err(io::Error::other(
            "getattrlistbulk name bytes exceed record length",
        ));
    }

    // OBJTYPE: u32 vtype.
    if pos + 4 > name_start || pos + 4 > record.len() {
        return Err(io::Error::other(
            "getattrlistbulk record too short for objtype",
        ));
    }
    let vtype = read_u32(record, pos);
    pos += 4;

    // MODTIME: struct timespec { i64 seconds, i64 nanoseconds }.
    let mut mtime_secs = 0u64;
    let mut mtime_nanos = 0u32;
    if common & ATTR_CMN_MODTIME != 0 {
        if pos + 16 > name_start || pos + 16 > record.len() {
            return Err(io::Error::other(
                "getattrlistbulk record too short for modtime",
            ));
        }
        let secs = read_i64(record, pos);
        let nanos = read_i64(record, pos + 8);
        pos += 16;
        if secs >= 0 {
            mtime_secs = secs as u64;
            mtime_nanos = nanos.clamp(0, 999_999_999) as u32;
        }
    }

    // ACCESSMASK: u32 FULL st_mode including the S_IFMT type bits;
    // consumers mask to the 0o7777 permission slice like they would
    // with `MetadataExt::mode`.
    let mut mode = 0u32;
    if common & ATTR_CMN_ACCESSMASK != 0 {
        if pos + 4 > name_start || pos + 4 > record.len() {
            return Err(io::Error::other(
                "getattrlistbulk record too short for accessmask",
            ));
        }
        mode = read_u32(record, pos);
        pos += 4;
    }

    // FILE DATALENGTH: u64; only returned for non-directories.
    let mut size = 0u64;
    if filebits & ATTR_FILE_DATALENGTH != 0 {
        if pos + 8 > name_start || pos + 8 > record.len() {
            return Err(io::Error::other(
                "getattrlistbulk record too short for datalength",
            ));
        }
        size = read_u64(record, pos);
        pos += 8;
    }

    // Ensure fixed attributes did not overrun into the name area
    if pos > name_start {
        return Err(io::Error::other(
            "getattrlistbulk attribute fields overrun name",
        ));
    }

    // Trailing NUL verification
    if record[name_start + name_len - 1] != 0 {
        return Err(io::Error::other(
            "getattrlistbulk name missing trailing NUL",
        ));
    }
    let raw_name = &record[name_start..name_start + name_len - 1];

    // Defensive: getattrlistbulk documents skipping "." and "..", but
    // a walk must never emit either as a real entry. Also reject
    // interior NULs sneaking through as empty names.
    if raw_name.is_empty() || raw_name.contains(&0) {
        return Ok(());
    }
    let name = String::from_utf8_lossy(raw_name);
    if name == "." || name == ".." {
        return Ok(());
    }
    let rel_path = if prefix.is_empty() {
        name.into_owned()
    } else {
        format!("{prefix}/{name}")
    };

    let is_dir = vtype == VDIR;
    let entry = BulkEntry {
        rel_path,
        is_dir,
        is_symlink: vtype == VLNK,
        is_file: vtype == VREG,
        size,
        mtime_secs,
        mtime_nanos,
        mode,
    };
    if is_dir {
        let child = dir.join(std::ffi::OsStr::from_bytes(raw_name));
        stack.push((child, entry.rel_path.clone()));
    }
    out.push(entry);
    Ok(())
}

/// Little-endian reads at arbitrary offsets: the kernel packs the
/// attrbuffer on 4-byte boundaries, which does not always leave
/// 8-byte fields naturally aligned. Bounds are enforced by the slice
/// indexing itself; callers length-check each record before parsing
/// it.
fn read_u32(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn read_i32(buf: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn read_u64(buf: &[u8], at: usize) -> u64 {
    u64::from_le_bytes([
        buf[at],
        buf[at + 1],
        buf[at + 2],
        buf[at + 3],
        buf[at + 4],
        buf[at + 5],
        buf[at + 6],
        buf[at + 7],
    ])
}

fn read_i64(buf: &[u8], at: usize) -> i64 {
    i64::from_le_bytes([
        buf[at],
        buf[at + 1],
        buf[at + 2],
        buf[at + 3],
        buf[at + 4],
        buf[at + 5],
        buf[at + 6],
        buf[at + 7],
    ])
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to encode a synthetic attrlistbulk record.
    fn encode_record(
        name: &str,
        vtype: u32,
        size: u64,
        mtime_secs: i64,
        mtime_nanos: i64,
        mode: u32,
        error_slot: u32,
    ) -> Vec<u8> {
        let mut rec = Vec::new();
        // placeholder for record_len (4 bytes)
        rec.extend_from_slice(&[0u8; 4]);
        // commonattr (4 bytes)
        rec.extend_from_slice(&REQUEST_COMMON.to_le_bytes());
        // volattr (4 bytes)
        rec.extend_from_slice(&0u32.to_le_bytes());
        // dirattr (4 bytes)
        rec.extend_from_slice(&0u32.to_le_bytes());
        // fileattr (4 bytes)
        let filebits = if vtype == VREG { REQUEST_FILE } else { 0 };
        rec.extend_from_slice(&filebits.to_le_bytes());
        // error_slot (4 bytes)
        rec.extend_from_slice(&error_slot.to_le_bytes());

        // Fixed fields:
        // pos 24: name ref (8 bytes) -> pos 32
        // pos 32: objtype (4 bytes) -> pos 36
        // pos 36: modtime (16 bytes) -> pos 52
        // pos 52: accessmask (4 bytes) -> pos 56
        // pos 56: datalength (8 bytes if filebits != 0) -> pos 64 (if file) or pos 56 (if non-file)
        let fixed_len = if filebits != 0 { 64 } else { 56 };
        let dataoffset: i32 = fixed_len - 24;
        let name_bytes = name.as_bytes();
        let name_len = (name_bytes.len() + 1) as u32;

        rec.extend_from_slice(&dataoffset.to_le_bytes());
        rec.extend_from_slice(&name_len.to_le_bytes());

        // objtype (4 bytes)
        rec.extend_from_slice(&vtype.to_le_bytes());

        // modtime (16 bytes)
        rec.extend_from_slice(&mtime_secs.to_le_bytes());
        rec.extend_from_slice(&mtime_nanos.to_le_bytes());

        // accessmask (4 bytes)
        rec.extend_from_slice(&mode.to_le_bytes());

        // datalength (8 bytes if filebits != 0)
        if filebits != 0 {
            rec.extend_from_slice(&size.to_le_bytes());
        }

        // Variable tail: name bytes + NUL
        rec.extend_from_slice(name_bytes);
        rec.push(0);

        // Pad to 4-byte multiple
        while rec.len() % 4 != 0 {
            rec.push(0);
        }

        let record_len = rec.len() as u32;
        rec[0..4].copy_from_slice(&record_len.to_le_bytes());
        rec
    }

    #[test]
    fn thread_local_scratch_buffer_initializes_and_reuses_across_calls() {
        let mut ptr1: *const u8 = std::ptr::null();
        with_scratch_buffer(|buf| {
            assert_eq!(buf.len(), INITIAL_BUFFER_CAPACITY);
            assert_eq!(buf.len(), 32 * 1024);
            ptr1 = buf.as_ptr();
        });

        let mut ptr2: *const u8 = std::ptr::null();
        with_scratch_buffer(|buf| {
            assert_eq!(buf.len(), INITIAL_BUFFER_CAPACITY);
            ptr2 = buf.as_ptr();
        });

        assert_eq!(
            ptr1, ptr2,
            "scratch buffer is reused across calls without reallocating"
        );
    }

    #[test]
    fn thread_local_scratch_buffer_thread_isolation() {
        use std::sync::mpsc::channel;

        let (tx, rx) = channel();
        let handle = std::thread::spawn(move || {
            with_scratch_buffer(|buf| {
                assert_eq!(buf.len(), INITIAL_BUFFER_CAPACITY);
                buf[0] = 42;
                tx.send(buf.as_ptr() as usize).unwrap();
            });
        });

        let child_ptr = rx.recv().unwrap();
        handle.join().unwrap();

        with_scratch_buffer(|buf| {
            assert_ne!(
                buf.as_ptr() as usize,
                child_ptr,
                "different threads receive independent scratch buffers"
            );
            assert_eq!(buf[0], 0, "main thread scratch buffer is isolated");
        });
    }

    #[test]
    fn dynamic_buffer_doubling_on_erange_and_1mb_ceiling() {
        with_scratch_buffer(|buf| {
            // Initial 32KB
            assert_eq!(buf.len(), 32 * 1024);

            // Simulate ERANGE doubling loop
            let mut cap = buf.len();
            while cap < MAX_BUFFER_CAPACITY {
                let new_cap = (cap * 2).min(MAX_BUFFER_CAPACITY);
                buf.resize(new_cap, 0);
                cap = buf.len();
            }

            assert_eq!(buf.len(), 1024 * 1024, "reaches exact 1MB ceiling");
            assert_eq!(buf.len(), MAX_BUFFER_CAPACITY);

            // Attempting to grow beyond ceiling is rejected
            let can_grow = buf.len() < MAX_BUFFER_CAPACITY;
            assert!(!can_grow, "must not grow past 1MB ceiling");
        });

        // Next call preserves the capacity without allocation
        with_scratch_buffer(|buf| {
            assert_eq!(buf.len(), MAX_BUFFER_CAPACITY);
        });
    }

    #[test]
    fn strict_parsing_emits_valid_records_with_high_fidelity() {
        let mut raw = Vec::new();
        let rec_file = encode_record("hello.txt", VREG, 4096, 1700000000, 500, 0o100644, 0);
        let rec_dir = encode_record("sub", VDIR, 0, 1700000010, 0, 0o040755, 0);
        let rec_link = encode_record("sym.lnk", VLNK, 0, 1700000020, 123, 0o120777, 0);

        raw.extend_from_slice(&rec_file);
        raw.extend_from_slice(&rec_dir);
        raw.extend_from_slice(&rec_link);

        let mut out = Vec::new();
        let mut stack = Vec::new();
        let dir = Path::new("/test/root");

        parse_bulk_buffer(&raw, 3, dir, "", &mut out, &mut stack).expect("valid bulk buffer");

        assert_eq!(out.len(), 3);
        assert_eq!(
            out[0],
            BulkEntry {
                rel_path: "hello.txt".into(),
                is_dir: false,
                is_symlink: false,
                is_file: true,
                size: 4096,
                mtime_secs: 1700000000,
                mtime_nanos: 500,
                mode: 0o100644,
            }
        );

        assert_eq!(
            out[1],
            BulkEntry {
                rel_path: "sub".into(),
                is_dir: true,
                is_symlink: false,
                is_file: false,
                size: 0,
                mtime_secs: 1700000010,
                mtime_nanos: 0,
                mode: 0o040755,
            }
        );
        assert_eq!(stack, vec![(dir.join("sub"), "sub".into())]);

        assert_eq!(
            out[2],
            BulkEntry {
                rel_path: "sym.lnk".into(),
                is_dir: false,
                is_symlink: true,
                is_file: false,
                size: 0,
                mtime_secs: 1700000020,
                mtime_nanos: 123,
                mode: 0o120777,
            }
        );
    }

    #[test]
    fn strict_parsing_prevents_ghost_entries_from_stale_buffer_tail() {
        let mut raw = Vec::new();
        let rec1 = encode_record("valid1.txt", VREG, 100, 1000, 0, 0o100644, 0);
        let rec2 = encode_record("valid2.txt", VREG, 200, 2000, 0, 0o100644, 0);
        let ghost1 = encode_record("ghost1.txt", VREG, 999, 9999, 0, 0o100644, 0);
        let ghost2 = encode_record("ghost2.txt", VREG, 888, 8888, 0, 0o100644, 0);

        raw.extend_from_slice(&rec1);
        raw.extend_from_slice(&rec2);
        raw.extend_from_slice(&ghost1);
        raw.extend_from_slice(&ghost2);

        let mut out = Vec::new();
        let mut stack = Vec::new();
        let dir = Path::new("/test/root");

        // Kernel returned entry count n = 2.
        parse_bulk_buffer(&raw, 2, dir, "prefix", &mut out, &mut stack).expect("parse buffer");

        assert_eq!(out.len(), 2, "only exactly n=2 entries are emitted");
        assert_eq!(out[0].rel_path, "prefix/valid1.txt");
        assert_eq!(out[1].rel_path, "prefix/valid2.txt");

        // Subsequent parse with n = 1
        out.clear();
        parse_bulk_buffer(&raw, 1, dir, "", &mut out, &mut stack).expect("parse buffer single");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rel_path, "valid1.txt");
    }

    #[test]
    fn strict_bounds_rejects_malformed_records() {
        let dir = Path::new("/test/root");
        let mut out = Vec::new();
        let mut stack = Vec::new();

        // 1. Buffer too short to read record_len (< 4 bytes)
        let short_buf = [1u8, 2, 3];
        assert!(parse_bulk_buffer(&short_buf, 1, dir, "", &mut out, &mut stack).is_err());

        // 2. Record length < 36 bytes
        let mut bad_len = encode_record("a", VREG, 0, 0, 0, 0, 0);
        bad_len[0..4].copy_from_slice(&20u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_len, 1, dir, "", &mut out, &mut stack).is_err());

        // 3. Record length unaligned (not a 4-byte multiple)
        bad_len[0..4].copy_from_slice(&41u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_len, 1, dir, "", &mut out, &mut stack).is_err());

        // 4. Record length exceeds buffer size
        bad_len[0..4].copy_from_slice(&1000u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_len, 1, dir, "", &mut out, &mut stack).is_err());

        // 5. Entry error slot is non-zero
        let err_slot = encode_record("err", VREG, 0, 0, 0, 0, libc::EIO as u32);
        assert!(parse_bulk_buffer(&err_slot, 1, dir, "", &mut out, &mut stack).is_err());

        // 6. Name offset out of bounds / corrupted
        let mut bad_name_off = encode_record("bad_offset", VREG, 0, 0, 0, 0, 0);
        bad_name_off[24..28].copy_from_slice(&(-10i32).to_le_bytes());
        assert!(parse_bulk_buffer(&bad_name_off, 1, dir, "", &mut out, &mut stack).is_err());

        // 7. Name length exceeds record length
        let mut bad_name_len = encode_record("overflow", VREG, 0, 0, 0, 0, 0);
        bad_name_len[28..32].copy_from_slice(&500u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_name_len, 1, dir, "", &mut out, &mut stack).is_err());

        // 8. Name missing trailing NUL
        let mut no_nul = encode_record("nonul", VREG, 0, 0, 0, 0, 0);
        let nul_idx = 64 + "nonul".len();
        no_nul[nul_idx] = b'X';
        assert!(parse_bulk_buffer(&no_nul, 1, dir, "", &mut out, &mut stack).is_err());

        // 9. Unexpected common attributes
        let mut unexp_attr = encode_record("unexp", VREG, 0, 0, 0, 0, 0);
        unexp_attr[4..8].copy_from_slice(&(REQUEST_COMMON | 0x0000_0002).to_le_bytes());
        assert!(parse_bulk_buffer(&unexp_attr, 1, dir, "", &mut out, &mut stack).is_err());
    }
}
