//! `wt remove` handler (arch-hardening ticket 03): the work happens
//! in `gc::remove`; this wrapper keeps command dispatch uniform.

use std::path::Path;

use crate::gc;

pub fn run(name: &str, dir: Option<&Path>) -> Result<(), String> {
    gc::remove(name, dir)
}
