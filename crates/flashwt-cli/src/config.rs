use std::env;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyPolicy {
    Default,

    Hardlink,

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

fn flag_with_default(name: &str, default: bool) -> bool {
    match env::var_os(name) {
        None => default,
        Some(value) => parse_bool_env(&value).unwrap_or(default),
    }
}

fn flag(name: &str) -> bool {
    flag_with_default(name, false)
}

fn probe_apfs_default() -> bool {
    #[cfg(target_os = "macos")]
    {
        let check_path = env::var_os("FLASHWT_STORE")
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

        flashwt_store::probe_fs(cur)
            .map(|c| c.reflink_capable)
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunConfig {
    pub strategy_policy: StrategyPolicy,

    pub verify: bool,

    pub snapshots: bool,

    pub v2: bool,

    pub timing: bool,

    pub json: bool,

    pub tiny_bypass: bool,
}

impl RunConfig {
    pub fn from_env() -> Self {
        let strategy_policy = if flag("FLASHWT_NO_HARDLINK") {
            StrategyPolicy::ForceByteCopy
        } else if flag("FLASHWT_HARDLINK") {
            StrategyPolicy::Hardlink
        } else {
            StrategyPolicy::Default
        };

        let apfs_default = probe_apfs_default();
        let snapshots = flag_with_default("FLASHWT_SNAPSHOTS", apfs_default);
        let v2 = flag_with_default("FLASHFLASHWT_SNAPSHOTS_V2", apfs_default);
        let tiny_bypass = if flag("FLASHWT_NO_TINY_BYPASS") {
            false
        } else {
            flag_with_default("FLASHWT_TINY_BYPASS", true)
        };

        RunConfig {
            strategy_policy,
            verify: flag("FLASHWT_VERIFY"),
            snapshots,
            v2,
            timing: flag("FLASHWT_TIMING"),
            json: false,
            tiny_bypass,
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        for v in [
            "0", "false", "FALSE", "False", "no", "NO", "off", "OFF", "Off",
        ] {
            set("FLASHWT_SNAPSHOTS", Some(v));
            assert!(
                !flag_with_default("FLASHWT_SNAPSHOTS", true),
                "value {v:?} should be false"
            );
            assert!(!flag("FLASHWT_SNAPSHOTS"), "flag {v:?} should be false");
        }

        for v in [
            "1", "true", "TRUE", "yes", "YES", "on", "ON", "anything", "2",
        ] {
            set("FLASHWT_SNAPSHOTS", Some(v));
            assert!(
                flag_with_default("FLASHWT_SNAPSHOTS", false),
                "value {v:?} should be true"
            );
            assert!(flag("FLASHWT_SNAPSHOTS"), "flag {v:?} should be true");
        }

        set("FLASHWT_SNAPSHOTS", Some(""));
        assert!(flag_with_default("FLASHWT_SNAPSHOTS", true));
        assert!(!flag_with_default("FLASHWT_SNAPSHOTS", false));
        assert!(
            !flag("FLASHWT_SNAPSHOTS"),
            "empty should be false for flag (default false)"
        );
        set("FLASHWT_SNAPSHOTS", Some("   "));
        assert!(flag_with_default("FLASHWT_SNAPSHOTS", true));

        set("FLASHWT_SNAPSHOTS", None);
        assert!(flag_with_default("FLASHWT_SNAPSHOTS", true));
        assert!(!flag_with_default("FLASHWT_SNAPSHOTS", false));
        assert!(!flag("FLASHWT_SNAPSHOTS"));
    }

    #[test]
    fn snapshots_false_disables() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set("FLASHWT_SNAPSHOTS", Some("false"));
        set("FLASHFLASHWT_SNAPSHOTS_V2", None);
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots, "FLASHWT_SNAPSHOTS=false should disable");
        set("FLASHWT_SNAPSHOTS", Some("no"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots);
        set("FLASHWT_SNAPSHOTS", Some("off"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots);
        set("FLASHWT_SNAPSHOTS", Some("0"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.snapshots);
        set("FLASHWT_SNAPSHOTS", Some(""));
        let cfg = RunConfig::from_env();

        assert_eq!(cfg.snapshots, probe_apfs_default());
        set("FLASHWT_SNAPSHOTS", None);
        let cfg = RunConfig::from_env();
        assert_eq!(cfg.snapshots, probe_apfs_default());

        set("FLASHWT_SNAPSHOTS", None);
    }

    #[test]
    fn tiny_bypass_flag_semantics() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set("FLASHWT_NO_TINY_BYPASS", None);
        set("FLASHWT_TINY_BYPASS", None);
        let cfg = RunConfig::from_env();
        assert!(cfg.tiny_bypass, "tiny bypass must default to true");

        set("FLASHWT_TINY_BYPASS", Some("0"));
        let cfg = RunConfig::from_env();
        assert!(!cfg.tiny_bypass, "FLASHWT_TINY_BYPASS=0 must disable");

        set("FLASHWT_TINY_BYPASS", Some("1"));
        set("FLASHWT_NO_TINY_BYPASS", Some("1"));
        let cfg = RunConfig::from_env();
        assert!(
            !cfg.tiny_bypass,
            "FLASHWT_NO_TINY_BYPASS=1 must take precedence"
        );

        set("FLASHWT_NO_TINY_BYPASS", None);
        set("FLASHWT_TINY_BYPASS", None);
    }
}
