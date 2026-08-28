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

/// Uniform tri-state flag semantics for boolean `wt_*` env vars with default value:
/// if unset, returns `default`; `"0"` means off (false); ANY other value
/// (including `"1"` and empty) means on (true).
fn flag_with_default(name: &str, default: bool) -> bool {
    match env::var_os(name) {
        None => default,
        Some(value) => value != "0",
    }
}

/// Uniform tri-state flag semantics for every boolean `wt_*` env var:
/// unset means off, `"0"` means off, and ANY other value — including
/// `"1"` and empty — means on. This replaces three different activation
/// semantics under which, notably, `WT_HARDLINK=0` turned hardlink
/// mode ON.
fn flag(name: &str) -> bool {
    flag_with_default(name, false)
}

/// Probe whether host filesystem supports APFS snapshot hydration by default.
fn probe_apfs_default() -> bool {
    #[cfg(target_os = "macos")]
    {
        let check_path = env::var_os("WT_STORE")
            .map(std::path::PathBuf::from)
            .or_else(|| env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("/"));

        let mut cur = check_path.as_path();
        while !cur.exists() {
            if let Some(parent) = cur.parent() {
                cur = parent;
            } else {
                break;
            }
        }

        wt_store::probe_fs(cur).map(|c| c.reflink_capable).unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The parsed environment policy for one process run.
#[derive(Debug, Clone, Copy)]
pub struct RunConfig {
    pub strategy_policy: StrategyPolicy,
    /// `WT_VERIFY`: re-hash every blob before placement; also bypasses
    /// snapshot hits entirely and rebuilds from freshly hashed blobs.
    pub verify: bool,
    /// `WT_SNAPSHOTS`: whole-directory snapshot fast path (defaults to enabled
    /// on macOS APFS; explicit `WT_SNAPSHOTS=0` opts out).
    pub snapshots: bool,
    /// `WT_SNAPSHOTS_V2`: incremental snapshot rebuilds (defaults to enabled
    /// on macOS APFS; explicit `WT_SNAPSHOTS_V2=0` opts out).
    pub v2: bool,
    /// `WT_TIMING`: print `wt-stage` lines to stderr.
    pub timing: bool,
    /// `--json`: emit single-line NDJSON output envelope.
    pub json: bool,
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

        let apfs_default = probe_apfs_default();
        let snapshots = flag_with_default("WT_SNAPSHOTS", apfs_default);
        let v2 = flag_with_default("WT_SNAPSHOTS_V2", apfs_default);

        RunConfig {
            strategy_policy,
            verify: flag("WT_VERIFY"),
            snapshots,
            v2,
            timing: flag("WT_TIMING"),
            json: false,
        }
    }
}
