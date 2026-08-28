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
    if !cfg.json {
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
    }
    let data = ScrubData {
        dry_run,
        scanned: report.scanned as u64,
        corrupt: report.corrupt.iter().map(|id| id.to_string()).collect(),
        deleted: report.deleted as u64,
    };
    Ok((data, diagnostics))
}
