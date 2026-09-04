mod disk;
mod fsutil;
mod gc;
mod ingest;
mod lease;
mod scrub;
mod snapdiff;
mod snapindex;
mod snapshot;
mod validation;
mod verified;

pub mod bulkwalk;
pub mod hydrate;
pub mod lockfile;
pub mod mirror;
pub use disk::{DiskStore, FsCapabilities, StoreDiskUsage, probe_fs};
pub use fsutil::FlockGuard;
pub use gc::{
    GcMode, MarkReport, MarkSwept, PendingCleanup, RetirementReceipt, StoreReclaimer, SweepPolicy,
    SweepSummary,
};
pub use hydrate::{
    HydrateDest, HydrateOutcome, HydratePinned, HydratePolicy, HydrateReq, HydrateSrc, HydrateTree,
    HydrationReceipt, WorkspaceHydrateReq, ZERO_SAVINGS_NO_FILES_HYDRATED,
    ZERO_SAVINGS_NO_MATCHING_DIRS, collect_matches, is_volatile_cache, pattern_matches,
};
pub use ingest::{IngestOptions, Ingested, stream_hash_file};
pub use lease::{
    DEFAULT_LEASE_TTL_SECS, ReadLease, WorktreeLease, current_process_start_time, is_lease_expired,
    is_process_alive, lease_path, process_start_time, publish as publish_lease,
    read_all as read_leases, remove as remove_lease,
};
pub use lockfile::{
    DependencySafety, LOCKFILES, classify_lockfile, find_lockfile, find_lockfile_rel,
    hash_lockfile, package_manager_command,
};
pub use mirror::{
    ReadMirror, StoreMirror, escape, mirror_path, publish as publish_mirror,
    read_all as read_mirrors, unescape, worktree_key,
};
pub use scrub::ScrubReport;
pub use snapdiff::SnapshotDiff;
pub use snapindex::{
    MAX_RING, MetadataLock, SelectionIndex, SelectionRecord, SnapshotLru, compact_journal,
    journal_path, lru_path, record_hit as record_snapshot_hit,
    record_publish as record_snapshot_publish, record_snapshot_lru_touch, select_old_snapshot,
};
pub use snapshot::{
    BuildError, EntryKind, INCREMENTAL_DIFF_RATIO_MAX, IncrementalDecision, IncrementalResult,
    Manifest, PublishOptions, PublishOutcome, PublishReceipt, SnapshotBuildTiming, SnapshotEntry,
    SnapshotHydration, SnapshotOutcome, SnapshotProjectionEngine, SnapshotProjectionRequest,
    read_published as read_published_snapshot, snapshot_path, snapshot_tree_path, try_incremental,
};
pub use validation::{Entry, ValidationCache};
pub use verified::{Fingerprint, VerifiedLedger};

use std::fmt;
use std::io;

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
    #[must_use]
    pub fn for_bytes(content: &[u8]) -> ContentId {
        use sha2::{Digest, Sha256};
        ContentId(Sha256::digest(content).into())
    }

    #[must_use]
    pub fn from_hex(text: &str) -> Option<ContentId> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 || !bytes.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, pair) in text.as_bytes().chunks(2).enumerate() {
            let (Some(hi), Some(lo)) = (
                (pair[0] as char).to_digit(16),
                (pair[1] as char).to_digit(16),
            ) else {
                return None;
            };
            out[i] = (hi as u8) << 4 | lo as u8;
        }
        Some(ContentId(out))
    }
}

#[derive(Debug)]
pub enum Error {
    Corrupted(ContentId),

    UnknownContent(ContentId),

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
