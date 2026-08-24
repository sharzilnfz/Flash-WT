//! `wt sweep` handler (arch-hardening ticket 03): the collection
//! schemes live in `gc::sweep`; this wrapper keeps command dispatch
//! uniform.

use crate::error::Result;
use crate::gc;

pub fn run(age: Option<&str>) -> Result<()> {
    gc::sweep(age)
}
