use std::time::Duration;

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, SweepData};
use crate::error::Result;

pub fn run(
    age: Option<Duration>,
    dry_run: bool,
    cfg: &RunConfig,
) -> Result<(SweepData, Vec<Diagnostic>)> {
    crate::gc::sweep(age, dry_run, cfg)
}
