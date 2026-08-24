//! `wt sweep` handler (arch-hardening ticket 03): the collection
//! schemes live in `gc::sweep`; this wrapper keeps command dispatch
//! uniform.

use crate::gc;

pub fn run(age: Option<&str>) -> Result<(), String> {
    gc::sweep(age)
}
