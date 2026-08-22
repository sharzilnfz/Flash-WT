//! Content-addressed store contract (ticket 01), implemented on disk
//! in ticket 04.
//!
//! The store is the source of truth (ADR-0001): every unique file
//! content is kept exactly once per machine, addressed by its hash.
//! Reference counting over stored content feeds garbage collection
//! (ticket 06).

mod disk;
mod gc;
mod mirror;
mod validation;
mod verified;

pub use disk::DiskStore;
pub use gc::{GcMode, MarkReport, MarkSwept};
pub use mirror::{
    escape, mirror_path, publish as publish_mirror, read_all as read_mirrors, unescape,
    worktree_key, ReadMirror, StoreMirror,
};
pub use validation::{Entry, ValidationCache};
pub use verified::{Fingerprint, VerifiedLedger};

use std::fmt;
use std::io;

/// Hash of one stored content blob. Fixed 32 bytes; the concrete hash
/// function is an implementation detail of ticket 04 and must not leak
/// past this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentId(pub [u8; 32]);

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl ContentId {
    /// Parse the lowercase hex form produced by [`Display`]. Returns
    /// `None` for anything that is not exactly 64 hex digits.
    pub fn from_hex(text: &str) -> Option<ContentId> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, pair) in text.as_bytes().chunks(2).enumerate() {
            let hi = (pair[0] as char).to_digit(16).expect("hex digit") as u8;
            let lo = (pair[1] as char).to_digit(16).expect("hex digit") as u8;
            out[i] = hi << 4 | lo;
        }
        Some(ContentId(out))
    }
}

#[derive(Debug)]
pub enum Error {
    /// `get` found the entry but its bytes no longer match the
    /// address. The store must surface corruption as this error, never
    /// return the bad bytes (spec: silent corruption is detectable).
    Corrupted(ContentId),
    /// The id is not in the store.
    UnknownContent(ContentId),
    /// A reference was released more times than it was added, or
    /// decremented on unknown content.
    RefCountUnderflow(ContentId),
    Io(io::Error),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Corrupted(id) => write!(f, "store content {id} failed hash verification"),
            Error::UnknownContent(id) => write!(f, "content {id} not in store"),
            Error::RefCountUnderflow(id) => write!(f, "reference count underflow for {id}"),
            Error::Io(e) => write!(f, "store io error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Hash-addressed content store with reference counting.
///
/// Contract for implementors (ticket 04):
///
/// - **Deduplication**: `put` with identical bytes always returns the
///   same [`ContentId`] and stores the data once on disk. Two puts of
///   the same content occupy disk space for exactly one copy.
/// - **References are explicit**: `put` does NOT take a reference. A
///   caller that wants to keep content alive across GC calls
///   [`Store::add_ref`]; it must pair every `add_ref` with exactly one
///   [`Store::release_ref`]. Releasing below zero is
///   [`Error::RefCountUnderflow`].
/// - **Integrity on read**: `get` recomputes the hash and returns
///   [`Error::Corrupted`] instead of returning mismatched bytes.
/// - **Durability**: implementations persist to a root directory they
///   own (constructor argument). Nothing outside that directory may be
///   written. Multiple `Store` handles on the same root within one
///   process must see each other's writes; cross-process locking can
///   wait for ticket 06.
pub trait Store {
    /// Store bytes, deduplicated by content. Returns the content's
    /// [`ContentId`]. Does not change any reference count.
    fn put(&mut self, content: &[u8]) -> Result<ContentId>;

    /// Fetch bytes by id, verifying the hash before returning.
    fn get(&self, id: &ContentId) -> Result<Vec<u8>>;

    /// True if the store holds this content. Does not verify the hash.
    fn contains(&self, id: &ContentId) -> bool;

    /// Increment the reference count. Errors with
    /// [`Error::UnknownContent`] if the id was never put.
    fn add_ref(&mut self, id: &ContentId) -> Result<()>;

    /// Decrement the reference count. Errors with
    /// [`Error::RefCountUnderflow`] if already at zero or absent.
    fn release_ref(&mut self, id: &ContentId) -> Result<()>;

    /// Current reference count (0 if stored but never referenced).
    fn ref_count(&self, id: &ContentId) -> Result<u64>;
}
