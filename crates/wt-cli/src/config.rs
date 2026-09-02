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

fn parse_bool_env(value: &std::ffi::OsStr) -> Option<bool> {
    let s = value.to_string_lossy();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => Some(false),
        "1" | "true" | "yes" | "on" => Some(true),
        _ => Some(true),
    }
}

/// Uniform tri-state flag semantics for boolean `wt_*` env vars with default value:
/// if unset or empty, returns `default`; `"0"`, `"false"`, `"no"`, `"off"`
/// (case-insensitive) mean off (false); any other non-empty value means on (true).
fn flag_with_default(name: &str, default: bool) -> bool {
    match env::var_os(name) {
        None => default,
        Some(value) => parse_bool_env(&value).unwrap_or(default),
    }
}

/// Uniform tri-state flag semantics for every boolean `wt_*` env var:
/// unset or empty means off, `"0"`, `"false"`, `"no"`, `"off"` (case-insensitive)
/// mean off, and any other non-empty value means on.
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

        wt_store::probe_fs(cur)
            .map(|c| c.reflink_capable)
            .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set(name: &str, val: Option<&str>) {
        match val {
            Some(v) => unsafe { env::set_var(name, v) },
            None => unsafe { env::remove_var(name) },
        }
    }

    #[test]
    fn flag_parsing_recognizes_false_variants_and_empty() {
        let _guard = ENV_LOCK.lock().unwrap();
        // false variants
        for v in [
            "0", "false", "FALSE", "False", "no", "NO", "off", "OFF", "Off",
        ] {
            set("WT_SNAPSHOTS", Some(v));
            assert!(
                !flag_with_default("WT_SNAPSHOTS", true),
                "value {v:?} should be false"
            );
            assert!(!flag("WT_SNAPSHOTS"), "flag {v:?} should be false");
        }
        // true variants
        for v in [
            "1", "true", "TRUE", "yes", "YES", "on", "ON", "anything", "2",
        ] {
            set("WT_SNAPSHOTS", Some(v));
            assert!(
                flag_with_default("WT_SNAPSHOTS", false),
                "value {v:?} should be true"
            );
            assert!(flag("WT_SNAPSHOTS"), "flag {v:?} should be true");
        }
        // empty treated as omitted/default
        set("WT_SNAPSHOTS", Some(""));
        assert!(flag_with_default("WT_SNAPSHOTS", true));
        assert!(!flag_with_default("WT_SNAPSHOTS", false));
        assert!(
            !flag("WT_SNAPSHOTS"),
            "empty should be false for flag (default false)"
        );
        set("WT_SNAPSHOTS", Some("   "));
        assert!(flag_with_default("WT_SNAPSHOTS", true));
        // unset
        set("WT_SNAPSHOTS", None);
        assert!(flag_with_default("WT_SNAPSHOTS", true));
        assert!(!flag_with_default("WT_SNAPSHOTS", false));
        assert!(!flag("WT_SNAPSHOTS"));
    }

    #[test]
    fn snapshots_false_disables() {
        let _guard = ENV_LOCK.lock().unwrap();
        set("WT_SNAPSHOTS", Some("false"));
        set("WT_SNAPSHOTS_V2", None);
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots, "WT_SNAPSHOTS=false should disable");
        set("WT_SNAPSHOTS", Some("no"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots);
        set("WT_SNAPSHOTS", Some("off"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots);
        set("WT_SNAPSHOTS", Some("0"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots);
        set("WT_SNAPSHOTS", Some(""));
        let cfg = RunConfig::from_env();
        // empty => default (probe_apfs_default())
        assert_eq!(cfg.snapshots, probe_apfs_default());
        set("WT_SNAPSHOTS", None);
        let cfg = RunConfig::from_env();
        assert_eq!(cfg.snapshots, probe_apfs_default());
        // cleanup
        set("WT_SNAPSHOTS", None);
    }
}
