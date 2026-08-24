//! `wt sweep` handler (arch-hardening ticket 03): the collection
//! schemes live in `gc::sweep`; this wrapper keeps command dispatch
//! uniform. Bad `--age` values never reach here — clap's value_parser
//! rejects them at parse time.

use std::time::Duration;

use crate::error::Result;
use crate::gc;

pub fn run(age: Option<Duration>) -> Result<()> {
    gc::sweep(age)
}
