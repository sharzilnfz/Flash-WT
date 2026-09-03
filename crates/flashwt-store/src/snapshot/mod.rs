mod manifest;
mod projection;
mod publish;
mod tree;

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests;

pub use manifest::{EntryKind, Manifest, SnapshotEntry};
pub use projection::{
    INCREMENTAL_DIFF_RATIO_MAX, IncrementalDecision, IncrementalResult, SnapshotHydration,
    SnapshotOutcome, SnapshotProjectionEngine, SnapshotProjectionRequest, try_incremental,
};
pub use publish::{
    BuildError, PublishOptions, PublishOutcome, PublishReceipt, SnapshotBuildTiming,
};
pub(crate) use tree::paranoid_verify_tree;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ContentId;

pub fn snapshot_path(root: &Path, hash: &ContentId) -> PathBuf {
    root.join("snapshots").join(hash.to_string())
}

pub fn snapshot_tree_path(root: &Path, hash: &ContentId) -> PathBuf {
    snapshot_path(root, hash).join("tree")
}

pub fn read_published(root: &Path, hash: &ContentId) -> Option<Manifest> {
    match read_published_checked(root, hash) {
        Verdict::Valid(manifest) => Some(manifest),
        Verdict::Miss | Verdict::Invalid(_) | Verdict::Io(_) => None,
    }
}

#[allow(dead_code)]
enum Verdict {
    Miss,

    Invalid(String),

    Io(io::Error),

    Valid(Manifest),
}

fn read_published_checked(root: &Path, hash: &ContentId) -> Verdict {
    let dir = snapshot_path(root, hash);
    let text = match fs::read_to_string(dir.join("manifest.tsv")) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Verdict::Miss,
        Err(e) => return Verdict::Io(e),
    };
    let manifest = match Manifest::parse(&text) {
        Ok(manifest) => manifest,
        Err(reason) => return Verdict::Invalid(format!("unparsable manifest: {reason}")),
    };
    if manifest.hash != *hash {
        return Verdict::Invalid("manifest hash does not match directory name".into());
    }
    let complete = match fs::read_to_string(dir.join(".complete")) {
        Ok(complete) => complete,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Verdict::Invalid("missing .complete marker".into());
        }
        Err(e) => return Verdict::Io(e),
    };
    let mut parts = complete.trim_end_matches('\n').split('\t');
    if parts.next() != Some("v1") {
        return Verdict::Invalid("wrong schema version in .complete".into());
    }
    let hash_text = hash.to_string();
    if parts.next() != Some(hash_text.as_str()) || parts.next().is_some() {
        return Verdict::Invalid(".complete does not match the manifest hash".into());
    }
    Verdict::Valid(manifest)
}
