//! Environment-derived run policy (arch-hardening ticket 03 item 3),
//! parsed ONCE at startup instead of scattered `std::env` reads at
//! arbitrary depths. Slices of this struct thread through the
//! hydration machinery, which therefore never reads those variables
//! itself.

use std::env;

/// Which placement strategy materialization uses, from the
/// `WT_HARDLINK` / `WT_NO_HARDLINK` pair (`WT_NO_HARDLINK` wins, the
/// historical precedence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyPolicy {
    /// Default: per-file CoW clones where the filesystem supports
    /// them, byte copies elsewhere.
    Default,
    /// Experimental hardlinked materialization: linked inodes are made
    /// read-only, so in-place rewrites fail loudly.
    Hardlink,
    /// Forced byte copies — the escape hatch.
    ForceByteCopy,
}

/// Uniform tri-state flag semantics for every boolean `wt_*` env var:
/// unset means off, `"0"` means off, and ANY other value — including
/// `"1"` and empty — means on. This replaces three different activation
/// semantics under which, notably, `WT_HARDLINK=0` turned hardlink
/// mode ON.
fn flag(name: &str) -> bool {
    match env::var_os(name) {
        None => false,
        Some(value) => value != "0",
    }
}

/// The parsed environment policy for one process run.
#[derive(Debug, Clone, Copy)]
pub struct RunConfig {
    pub strategy_policy: StrategyPolicy,
    /// `WT_VERIFY`: re-hash every blob before placement; also bypasses
    /// snapshot hits entirely and rebuilds from freshly hashed blobs.
    pub verify: bool,
    /// `WT_SNAPSHOTS`: whole-directory snapshot fast path (macOS/APFS
    /// only in practice; other platforms fall back to the ladder).
    pub snapshots: bool,
    /// `WT_SNAPSHOTS_V2`: incremental snapshot rebuilds. Only ever
    /// honored together with `snapshots`.
    #[cfg_attr(
        not(target_os = "macos"),
        expect(dead_code, reason = "v2 gate is macOS-only")
    )]
    pub v2: bool,
    /// `WT_TIMING`: print `wt-stage` lines to stderr.
    pub timing: bool,
}

impl RunConfig {
    pub fn from_env() -> Self {
        let strategy_policy = if flag("WT_NO_HARDLINK") {
            StrategyPolicy::ForceByteCopy
        } else if flag("WT_HARDLINK") {
            StrategyPolicy::Hardlink
        } else {
            StrategyPolicy::Default
        };
        RunConfig {
            strategy_policy,
            verify: flag("WT_VERIFY"),
            snapshots: flag("WT_SNAPSHOTS"),
            v2: flag("WT_SNAPSHOTS_V2"),
            timing: flag("WT_TIMING"),
        }
    }
}
