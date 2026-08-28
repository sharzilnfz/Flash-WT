use std::time::Duration;

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, SweepData};
use crate::error::Result;
use crate::gc;

pub fn run(age: Option<Duration>, cfg: &RunConfig) -> Result<(SweepData, Vec<Diagnostic>)> {
    gc::sweep(age, cfg)
}
