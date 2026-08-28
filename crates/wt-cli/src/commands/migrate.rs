use crate::config::RunConfig;
use crate::envelope::{Diagnostic, MigrateData};
use crate::error::Result;
use crate::gc;

pub fn run(
    activate_mark_sweep: bool,
    drop_legacy_refs: bool,
    cfg: &RunConfig,
) -> Result<(MigrateData, Vec<Diagnostic>)> {
    gc::migrate(activate_mark_sweep, drop_legacy_refs, cfg)
}
