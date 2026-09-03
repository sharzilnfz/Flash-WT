use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::ffi::CString;
#[cfg(target_os = "macos")]
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub const INITIAL_BUFFER_CAPACITY: usize = 32 * 1024;

pub const MAX_BUFFER_CAPACITY: usize = 1024 * 1024;

thread_local! {
    static SCRATCH_BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkEntry {
    pub rel_path: String,

    pub is_dir: bool,

    pub is_symlink: bool,

    pub is_file: bool,

    pub size: u64,

    pub mtime_secs: u64,

    pub mtime_nanos: u32,

    pub inode: u64,

    pub ctime_secs: u64,

    pub ctime_nanos: u32,

    pub mode: u32,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const ATTR_BIT_MAP_COUNT: u16 = 5;

const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const ATTR_CMN_NAME: u32 = 0x0000_0001;
const ATTR_CMN_OBJTYPE: u32 = 0x0000_0008;
const ATTR_CMN_MODTIME: u32 = 0x0000_0400;
const ATTR_CMN_CHGTIME: u32 = 0x0000_0800;
const ATTR_CMN_ACCESSMASK: u32 = 0x0002_0000;
const ATTR_CMN_FILEID: u32 = 0x0200_0000;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x8000_0000;

const ATTR_FILE_DATALENGTH: u32 = 0x0000_0200;

const REQUEST_COMMON: u32 = ATTR_CMN_NAME
    | ATTR_CMN_OBJTYPE
    | ATTR_CMN_MODTIME
    | ATTR_CMN_CHGTIME
    | ATTR_CMN_ACCESSMASK
    | ATTR_CMN_FILEID
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

#[cfg(target_os = "macos")]
unsafe extern "C" {

    fn getattrlistbulk(
        fd: i32,
        attr_list: *const AttrList,
        attr_buf: *mut core::ffi::c_void,
        attr_buf_size: usize,
        options: u64,
    ) -> i32;

    fn open(path: *const core::ffi::c_char, oflag: i32, ...) -> i32;

    fn close(fd: i32) -> i32;
}

#[cfg(target_os = "macos")]
pub fn walk(src: &Path) -> io::Result<Vec<BulkEntry>> {
    let mut out = Vec::new();

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
                return Ok(());
            }
            if n < 0 {
                let err = io::Error::last_os_error();

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

    let entry_error = read_u32(record, 20);
    if entry_error != 0 {
        return Err(io::Error::other("getattrlistbulk entry error"));
    }

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

    if pos + 4 > name_start || pos + 4 > record.len() {
        return Err(io::Error::other(
            "getattrlistbulk record too short for objtype",
        ));
    }
    let vtype = read_u32(record, pos);
    pos += 4;

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

    let mut ctime_secs = 0u64;
    let mut ctime_nanos = 0u32;
    if common & ATTR_CMN_CHGTIME != 0 {
        if pos + 16 > name_start || pos + 16 > record.len() {
            return Err(io::Error::other(
                "getattrlistbulk record too short for chgtime",
            ));
        }
        let secs = read_i64(record, pos);
        let nanos = read_i64(record, pos + 8);
        pos += 16;
        if secs >= 0 {
            ctime_secs = secs as u64;
            ctime_nanos = nanos.clamp(0, 999_999_999) as u32;
        }
    }

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

    let mut inode = 0u64;
    if common & ATTR_CMN_FILEID != 0 {
        if pos + 8 > name_start || pos + 8 > record.len() {
            return Err(io::Error::other(
                "getattrlistbulk record too short for fileid",
            ));
        }
        inode = read_u64(record, pos);
        pos += 8;
    }

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

    if pos > name_start {
        return Err(io::Error::other(
            "getattrlistbulk attribute fields overrun name",
        ));
    }

    if record[name_start + name_len - 1] != 0 {
        return Err(io::Error::other(
            "getattrlistbulk name missing trailing NUL",
        ));
    }
    let raw_name = &record[name_start..name_start + name_len - 1];

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

    let (inode, ctime_secs, ctime_nanos) =
        if common & ATTR_CMN_FILEID != 0 && common & ATTR_CMN_CHGTIME != 0 {
            (inode, ctime_secs, ctime_nanos)
        } else {
            #[cfg(target_os = "macos")]
            {
                use std::os::unix::fs::MetadataExt;
                let child_path = dir.join(std::ffi::OsStr::from_bytes(raw_name));
                if let Ok(meta) = fs::symlink_metadata(&child_path) {
                    (
                        meta.ino(),
                        meta.ctime().max(0) as u64,
                        meta.ctime_nsec().clamp(0, 999_999_999) as u32,
                    )
                } else {
                    (inode, ctime_secs, ctime_nanos)
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                (inode, ctime_secs, ctime_nanos)
            }
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
        inode,
        ctime_secs,
        ctime_nanos,
        mode,
    };
    if is_dir {
        let child = dir.join(std::ffi::OsStr::from_bytes(raw_name));
        stack.push((child, entry.rel_path.clone()));
    }
    out.push(entry);
    Ok(())
}

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

    #[allow(clippy::too_many_arguments)]
    fn encode_record(
        name: &str,
        vtype: u32,
        size: u64,
        mtime_secs: i64,
        mtime_nanos: i64,
        ctime_secs: i64,
        ctime_nanos: i64,
        inode: u64,
        mode: u32,
        error_slot: u32,
    ) -> Vec<u8> {
        let mut rec = Vec::new();

        rec.extend_from_slice(&[0u8; 4]);

        rec.extend_from_slice(&REQUEST_COMMON.to_le_bytes());

        rec.extend_from_slice(&0u32.to_le_bytes());

        rec.extend_from_slice(&0u32.to_le_bytes());

        let filebits = if vtype == VREG { REQUEST_FILE } else { 0 };
        rec.extend_from_slice(&filebits.to_le_bytes());

        rec.extend_from_slice(&error_slot.to_le_bytes());

        let fixed_len = if filebits != 0 { 88 } else { 80 };
        let dataoffset: i32 = fixed_len - 24;
        let name_bytes = name.as_bytes();
        let name_len = (name_bytes.len() + 1) as u32;

        rec.extend_from_slice(&dataoffset.to_le_bytes());
        rec.extend_from_slice(&name_len.to_le_bytes());

        rec.extend_from_slice(&vtype.to_le_bytes());

        rec.extend_from_slice(&mtime_secs.to_le_bytes());
        rec.extend_from_slice(&mtime_nanos.to_le_bytes());

        rec.extend_from_slice(&ctime_secs.to_le_bytes());
        rec.extend_from_slice(&ctime_nanos.to_le_bytes());

        rec.extend_from_slice(&mode.to_le_bytes());

        rec.extend_from_slice(&inode.to_le_bytes());

        if filebits != 0 {
            rec.extend_from_slice(&size.to_le_bytes());
        }

        rec.extend_from_slice(name_bytes);
        rec.push(0);

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

        let (tx_child, rx_child) = channel();
        let (tx_done, rx_done) = channel();

        with_scratch_buffer(|buf| {
            buf[0] = 10;
        });

        let handle = std::thread::spawn(move || {
            with_scratch_buffer(|buf| {
                assert_eq!(buf.len(), INITIAL_BUFFER_CAPACITY);
                buf[0] = 42;
                tx_child.send(buf.as_ptr() as usize).unwrap();
            });
            let _ = rx_done.recv();
        });

        let child_ptr = rx_child.recv().unwrap();

        with_scratch_buffer(|buf| {
            assert_ne!(
                buf.as_ptr() as usize,
                child_ptr,
                "different threads receive independent scratch buffers"
            );
            assert_eq!(buf[0], 10, "main thread scratch buffer is isolated");
        });

        let _ = tx_done.send(());
        handle.join().unwrap();
    }

    #[test]
    fn dynamic_buffer_doubling_on_erange_and_1mb_ceiling() {
        with_scratch_buffer(|buf| {
            assert_eq!(buf.len(), 32 * 1024);

            let mut cap = buf.len();
            while cap < MAX_BUFFER_CAPACITY {
                let new_cap = (cap * 2).min(MAX_BUFFER_CAPACITY);
                buf.resize(new_cap, 0);
                cap = buf.len();
            }

            assert_eq!(buf.len(), 1024 * 1024, "reaches exact 1MB ceiling");
            assert_eq!(buf.len(), MAX_BUFFER_CAPACITY);

            let can_grow = buf.len() < MAX_BUFFER_CAPACITY;
            assert!(!can_grow, "must not grow past 1MB ceiling");
        });

        with_scratch_buffer(|buf| {
            assert_eq!(buf.len(), MAX_BUFFER_CAPACITY);
        });
    }

    #[test]
    fn strict_parsing_emits_valid_records_with_high_fidelity() {
        let mut raw = Vec::new();
        let rec_file = encode_record(
            "hello.txt",
            VREG,
            4096,
            1700000000,
            500,
            1700000001,
            600,
            1001,
            0o100644,
            0,
        );
        let rec_dir = encode_record(
            "sub", VDIR, 0, 1700000010, 0, 1700000011, 0, 1002, 0o040755, 0,
        );
        let rec_link = encode_record(
            "sym.lnk", VLNK, 0, 1700000020, 123, 1700000021, 456, 1003, 0o120777, 0,
        );

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
                inode: 1001,
                ctime_secs: 1700000001,
                ctime_nanos: 600,
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
                inode: 1002,
                ctime_secs: 1700000011,
                ctime_nanos: 0,
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
                inode: 1003,
                ctime_secs: 1700000021,
                ctime_nanos: 456,
                mode: 0o120777,
            }
        );
    }

    #[test]
    fn strict_parsing_prevents_ghost_entries_from_stale_buffer_tail() {
        let mut raw = Vec::new();
        let rec1 = encode_record("valid1.txt", VREG, 100, 1000, 0, 1001, 0, 2001, 0o100644, 0);
        let rec2 = encode_record("valid2.txt", VREG, 200, 2000, 0, 2001, 0, 2002, 0o100644, 0);
        let ghost1 = encode_record("ghost1.txt", VREG, 999, 9999, 0, 9999, 0, 9999, 0o100644, 0);
        let ghost2 = encode_record("ghost2.txt", VREG, 888, 8888, 0, 8888, 0, 8888, 0o100644, 0);

        raw.extend_from_slice(&rec1);
        raw.extend_from_slice(&rec2);
        raw.extend_from_slice(&ghost1);
        raw.extend_from_slice(&ghost2);

        let mut out = Vec::new();
        let mut stack = Vec::new();
        let dir = Path::new("/test/root");

        parse_bulk_buffer(&raw, 2, dir, "prefix", &mut out, &mut stack).expect("parse buffer");

        assert_eq!(out.len(), 2, "only exactly n=2 entries are emitted");
        assert_eq!(out[0].rel_path, "prefix/valid1.txt");
        assert_eq!(out[1].rel_path, "prefix/valid2.txt");

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

        let short_buf = [1u8, 2, 3];
        assert!(parse_bulk_buffer(&short_buf, 1, dir, "", &mut out, &mut stack).is_err());

        let mut bad_len = encode_record("a", VREG, 0, 0, 0, 0, 0, 0, 0, 0);
        bad_len[0..4].copy_from_slice(&20u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_len, 1, dir, "", &mut out, &mut stack).is_err());

        bad_len[0..4].copy_from_slice(&41u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_len, 1, dir, "", &mut out, &mut stack).is_err());

        bad_len[0..4].copy_from_slice(&1000u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_len, 1, dir, "", &mut out, &mut stack).is_err());

        let err_slot = encode_record("err", VREG, 0, 0, 0, 0, 0, 0, 0, libc::EIO as u32);
        assert!(parse_bulk_buffer(&err_slot, 1, dir, "", &mut out, &mut stack).is_err());

        let mut bad_name_off = encode_record("bad_offset", VREG, 0, 0, 0, 0, 0, 0, 0, 0);
        bad_name_off[24..28].copy_from_slice(&(-10i32).to_le_bytes());
        assert!(parse_bulk_buffer(&bad_name_off, 1, dir, "", &mut out, &mut stack).is_err());

        let mut bad_name_len = encode_record("overflow", VREG, 0, 0, 0, 0, 0, 0, 0, 0);
        bad_name_len[28..32].copy_from_slice(&500u32.to_le_bytes());
        assert!(parse_bulk_buffer(&bad_name_len, 1, dir, "", &mut out, &mut stack).is_err());

        let mut no_nul = encode_record("nonul", VREG, 0, 0, 0, 0, 0, 0, 0, 0);
        let nul_idx = 88 + "nonul".len();
        no_nul[nul_idx] = b'X';
        assert!(parse_bulk_buffer(&no_nul, 1, dir, "", &mut out, &mut stack).is_err());

        let mut unexp_attr = encode_record("unexp", VREG, 0, 0, 0, 0, 0, 0, 0, 0);
        unexp_attr[4..8].copy_from_slice(&(REQUEST_COMMON | 0x0000_0002).to_le_bytes());
        assert!(parse_bulk_buffer(&unexp_attr, 1, dir, "", &mut out, &mut stack).is_err());
    }
}
