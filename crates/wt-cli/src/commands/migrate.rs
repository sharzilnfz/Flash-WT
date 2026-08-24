//! `wt store migrate` handler (arch-hardening ticket 03): the cutover
//! itself lives in `gc::migrate`; the exactly-one-flag rule is owned
//! by clap (required `migrate-mode` ArgGroup).

use crate::error::Result;
use crate::gc;

pub fn run(activate_mark_sweep: bool, drop_legacy_refs: bool) -> Result<()> {
    gc::migrate(activate_mark_sweep, drop_legacy_refs)
}
