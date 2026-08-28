use std::path::Path;

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, RemoveData};
use crate::error::Result;
use crate::gc;

pub fn run(
    name: &str,
    dir: Option<&Path>,
    cfg: &RunConfig,
) -> Result<(RemoveData, Vec<Diagnostic>)> {
    gc::remove(name, dir, cfg)
}
