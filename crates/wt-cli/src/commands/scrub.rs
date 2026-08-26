//! `wt scrub` handler: the hash-and-repair pass lives in wt-store
//! ([`DiskStore::scrub`]); this wrapper owns the human-readable
//! reporting — one stderr line per corrupt blob, then a summary in
//! the same style as `wt sweep`.

use crate::error::Result;
use crate::hydrate::open_store;

pub fn run(dry_run: bool) -> Result<()> {
    let mut store = open_store()?;
    let report = store.scrub(dry_run)?;
    for id in &report.corrupt {
        eprintln!("wt-scrub: blob {id} no longer matches its address");
    }
    if dry_run {
        println!(
            "scrubbed store (dry run): scanned {}, corrupt {}, would delete {}",
            report.scanned,
            report.corrupt.len(),
            report.corrupt.len()
        );
    } else {
        println!(
            "scrubbed store: scanned {}, corrupt {}, deleted {}",
            report.scanned,
            report.corrupt.len(),
            report.deleted
        );
    }
    Ok(())
}
