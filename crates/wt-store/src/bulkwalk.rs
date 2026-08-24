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
//! `read_dir`+`metadata` walk unchanged. Non-macOS builds never see
//! this module.
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

#![cfg(target_os = "macos")]

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// One walked filesystem entry, relative to the walk root.
///
/// Mirrors what the legacy walk learns from `DirEntry::file_type()`
/// plus one `fs::metadata()` per regular file — except symlinks are
/// never followed and their targets are left for the caller to read.
#[derive(Debug, Clone)]
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

const O_DIRECTORY: i32 = 0x0010_0000;
const O_CLOEXEC: i32 = 0x0100_0000;

#[repr(C)]
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
extern "C" {
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
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = unsafe { getattrlistbulk(fd, &attr_list, buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n == 0 {
            return Ok(()); // end of directory
        }
        if n < 0 {
            let err = io::Error::last_os_error();
            // One entry did not fit the buffer: retry with a bigger one
            // rather than giving up on the whole walk, up to a sane
            // ceiling (then degrade to the portable walk).
            if err.raw_os_error() == Some(libc::ERANGE) && buf.len() < 1024 * 1024 {
                buf.resize(buf.len() * 2, 0);
                continue;
            }
            return Err(err);
        }
        let mut offset = 0usize;
        for _ in 0..n {
            if offset + 24 > buf.len() {
                return Err(io::Error::other("getattrlistbulk buffer overrun"));
            }
            let record_len = read_u32(&buf, offset) as usize;
            if record_len < 32 || offset + record_len > buf.len() {
                return Err(io::Error::other(
                    "getattrlistbulk returned a malformed record",
                ));
            }
            let record = &buf[offset..offset + record_len];
            offset += record_len;
            parse_record(record, dir, prefix, out, stack)?;
        }
    }
}

/// Decode one attrbuffer record: 4-byte length, 16-byte
/// `attribute_set_t` of returned bitmaps, then the fixed-width data
/// for exactly the attributes we requested, in the kernel's source
/// order (see module docs), then the name bytes in the variable tail.
fn parse_record(
    record: &[u8],
    dir: &Path,
    prefix: &str,
    out: &mut Vec<BulkEntry>,
    stack: &mut Vec<(PathBuf, String)>,
) -> io::Result<()> {
    let common = read_u32(record, 4);
    let filebits = read_u32(record, 16);

    // Anything beyond what we asked for means the layout assumptions
    // above are void; bail into the portable fallback.
    if common & !REQUEST_COMMON != 0 || filebits & !REQUEST_FILE != 0 {
        return Err(io::Error::other("unexpected attributes in attrbuffer"));
    }
    if common & ATTR_CMN_NAME == 0 || common & ATTR_CMN_OBJTYPE == 0 {
        return Err(io::Error::other("getattrlistbulk returned no name/type"));
    }

    // Fixed-width data, consumed in kernel source order. A reserved
    // (always-zero here) error slot sits at @20 before the first
    // attribute; see get_error_attributes in xnu's vfs_attrlist.c.
    let mut pos = 24usize;

    // NAME: attrreference_t { i32 dataoffset (relative to itself),
    // u32 length including the trailing NUL }; bytes live in the tail.
    let dataoffset = read_i32(record, pos);
    let name_len = read_u32(record, pos + 4) as usize;
    if name_len == 0 || dataoffset < 0 {
        return Err(io::Error::other("getattrlistbulk returned a bad name"));
    }
    let name_start = (pos as isize + dataoffset as isize) as usize;
    if name_start + name_len > record.len() {
        return Err(io::Error::other("getattrlistbulk returned a bad name"));
    }
    let raw_name = &record[name_start..name_start + name_len - 1];
    pos += 8;

    // OBJTYPE: u32 vtype.
    let vtype = read_u32(record, pos);
    pos += 4;

    // MODTIME: struct timespec { i64 seconds, i64 nanoseconds }.
    let mut mtime_secs = 0u64;
    let mut mtime_nanos = 0u32;
    if common & ATTR_CMN_MODTIME != 0 {
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
        mode = read_u32(record, pos);
        pos += 4;
    }

    // FILE DATALENGTH: u64; only returned for non-directories.
    let mut size = 0u64;
    if filebits & ATTR_FILE_DATALENGTH != 0 {
        size = read_u64(record, pos);
    }

    // Defensive: getattrlistbulk documents skipping "." and "..", but
    // a walk must never emit either as a real entry. Also reject
    // interior NULs sneaking through as empty names.
    let name = String::from_utf8_lossy(raw_name);
    if name.is_empty() || name == "." || name == ".." {
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
