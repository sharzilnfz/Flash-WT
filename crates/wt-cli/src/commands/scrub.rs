use crate::config::RunConfig;
use crate::envelope::{Diagnostic, ScrubData};
use crate::error::Result;
use crate::hydrate::open_store;

pub fn run(dry_run: bool, cfg: &RunConfig) -> Result<(ScrubData, Vec<Diagnostic>)> {
    let mut store = open_store()?;
    let report = store.scrub(dry_run)?;
    let mut diagnostics = Vec::new();
    for id in &report.corrupt {
        eprintln!("wt-scrub: blob {id} no longer matches its address");
        diagnostics.push(Diagnostic::warning(
            "CORRUPT_BLOB",
            format!("blob {id} no longer matches its address"),
        ));
    }
    for item in &report.corrupt_snapshots {
        eprintln!("wt-scrub: snapshot {item} is broken or corrupted");
        diagnostics.push(Diagnostic::warning(
            "CORRUPT_SNAPSHOT",
            format!("snapshot {item} is broken or corrupted"),
        ));
    }
    if !cfg.json {
        if dry_run {
            if report.snapshot_dirs_scanned > 0 || !report.corrupt_snapshots.is_empty() {
                println!(
                    "scrubbed store (dry run): scanned {}, corrupt {}, would delete {}; snapshots: scanned {}, broken {}, would delete {}",
                    report.scanned,
                    report.corrupt.len(),
                    report.corrupt.len(),
                    report.snapshot_dirs_scanned,
                    report.corrupt_snapshots.len(),
                    report.corrupt_snapshots.len()
                );
            } else {
                println!(
                    "scrubbed store (dry run): scanned {}, corrupt {}, would delete {}",
                    report.scanned,
                    report.corrupt.len(),
                    report.corrupt.len()
                );
            }
        } else {
            if report.snapshot_dirs_scanned > 0 || !report.corrupt_snapshots.is_empty() {
                println!(
                    "scrubbed store: scanned {}, corrupt {}, deleted {}; snapshots: scanned {}, broken {}, deleted {}",
                    report.scanned,
                    report.corrupt.len(),
                    report.deleted,
                    report.snapshot_dirs_scanned,
                    report.corrupt_snapshots.len(),
                    report.snapshot_dirs_deleted
                );
            } else {
                println!(
                    "scrubbed store: scanned {}, corrupt {}, deleted {}",
                    report.scanned,
                    report.corrupt.len(),
                    report.deleted
                );
            }
        }
    }
    let data = ScrubData {
        dry_run,
        scanned: report.scanned as u64,
        corrupt: report.corrupt.iter().map(|id| id.to_string()).collect(),
        deleted: report.deleted as u64,
        snapshot_dirs_scanned: if report.snapshot_dirs_scanned > 0 || !report.corrupt_snapshots.is_empty() {
            Some(report.snapshot_dirs_scanned)
        } else {
            None
        },
        corrupt_snapshots: if report.snapshot_dirs_scanned > 0 || !report.corrupt_snapshots.is_empty() {
            Some(report.corrupt_snapshots.clone())
        } else {
            None
        },
        snapshot_dirs_deleted: if report.snapshot_dirs_scanned > 0 || !report.corrupt_snapshots.is_empty() {
            Some(report.snapshot_dirs_deleted)
        } else {
            None
        },
    };
    Ok((data, diagnostics))
}
