//! `wt store migrate` handler (arch-hardening ticket 03): the cutover
//! itself lives in `gc::migrate`.

use crate::gc;

pub fn run(activate_mark_sweep: bool, drop_legacy_refs: bool) -> Result<(), String> {
    if activate_mark_sweep == drop_legacy_refs {
        return Err("choose exactly one of --activate-mark-sweep or --drop-legacy-refs".into());
    }
    gc::migrate(activate_mark_sweep, drop_legacy_refs)
}
