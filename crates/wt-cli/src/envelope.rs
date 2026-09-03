//! Versioned JSON output envelope and command payload schemas
//! (ticket 01 / agent workflows).

use serde::{Deserialize, Serialize};

/// Current JSON output schema version (ticket 01).
pub const SCHEMA_VERSION: u32 = 1;

/// Structured diagnostic item within an envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            level: Some("error".into()),
        }
    }

    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            level: Some("warning".into()),
        }
    }
}

/// Generic versioned JSON response envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope<T> {
    pub wt_version: String,
    pub schema_version: u32,
    pub command: String,
    pub status: String,
    pub data: Option<T>,
    pub diagnostics: Vec<Diagnostic>,
}

impl<T: Serialize> Envelope<T> {
    pub fn ok(command: impl Into<String>, data: T, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            wt_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: SCHEMA_VERSION,
            command: command.into(),
            status: "ok".to_string(),
            data: Some(data),
            diagnostics,
        }
    }

    pub fn error(command: impl Into<String>, diagnostics: Vec<Diagnostic>) -> Envelope<()> {
        Envelope {
            wt_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: SCHEMA_VERSION,
            command: command.into(),
            status: "error".to_string(),
            data: None,
            diagnostics,
        }
    }
}

/// Payload for `wt create --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateData {
    pub worktree_path: String,
    pub branch: String,
    pub cache_hit: bool,
    pub duration_ms: u64,
    pub hydration_method: String,
    pub bytes_shared_cow: u64,
    pub bytes_copied: u64,
    pub files_hydrated: usize,
}

/// Payload for `wt hydrate --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HydrateData {
    pub destination_path: String,
    pub source_path: String,
    pub cache_hit: bool,
    pub duration_ms: u64,
    pub hydration_method: String,
    pub bytes_shared_cow: u64,
    pub bytes_copied: u64,
    pub files_hydrated: usize,
    pub dirs_hydrated: Vec<String>,
}

/// Payload for `wt init --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitData {
    pub manifest_path: String,
    pub created: bool,
}

/// Payload for `wt remove --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoveData {
    pub worktree_path: String,
    pub branch: String,
    pub references_released: usize,
    pub mirror_removed: bool,
}

/// Payload for `wt sweep --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SweepData {
    pub mode: String,
    pub examined: usize,
    pub reclaimed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mirrors_removed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_dirs_removed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_cap_evicted: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deferred_by_grace: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leases_examined: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leases_reclaimed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_bytes_reclaimed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreferenced_blobs: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dead_leases: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reclaimed_bytes: Option<u64>,
}

/// Payload for `wt scrub --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScrubData {
    pub dry_run: bool,
    pub scanned: u64,
    pub corrupt: Vec<String>,
    pub deleted: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_dirs_scanned: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corrupt_snapshots: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_dirs_deleted: Option<u64>,
}

/// Payload for `wt store migrate --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrateData {
    pub gc_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purged_legacy_refs: Option<usize>,
}

/// Payload for `wt store du --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreDuData {
    pub store_path: String,
    pub objects_bytes: u64,
    pub snapshots_bytes: u64,
    pub mirrors_bytes: u64,
    pub refs_bytes: u64,
    pub caches_bytes: u64,
    pub total_bytes: u64,
}

/// Payload for `wt doctor --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorData {
    pub store_path: String,
    pub env_vars: DoctorEnvVars,
    pub fs_capabilities: DoctorFsCapabilities,
    pub store_disk_usage: StoreDuData,
}

/// Environment variable diagnostics in `DoctorData`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorEnvVars {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_store: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_snapshots: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_snapshots_v2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_verify: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_timing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_gc_grace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_snapshot_cap: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_max_snapshot_bytes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_hardlink: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wt_no_hardlink: Option<String>,
}

/// Probed filesystem capabilities in `DoctorData`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorFsCapabilities {
    pub apfs_clonefile: bool,
    pub ficlone: bool,
    pub copy_file_range: bool,
}

/// Payload for `wt list --json` / `wt ls --json` (ticket 02).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListData {
    pub worktrees: Vec<WorktreeEntry>,
    pub total_disk_saved: u64,
    pub total_files_hydrated: usize,
}

/// Individual worktree entry within `ListData`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: String,
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub is_active: bool,
    pub is_main: bool,
    pub is_ephemeral: bool,
    pub files_hydrated: usize,
    pub bytes_hydrated: u64,
    pub bytes_saved: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hydrated_dirs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<LeaseEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_secs: Option<u64>,
}

/// Ephemeral scratch lease details for a worktree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LeaseEntry {
    pub lease_id: String,
    pub pid: u32,
    pub pid_alive: bool,
    pub expires_at: u64,
    pub ttl_remaining_secs: u64,
    pub is_expired: bool,
}

/// Payload for `wt scratch --json` / `wt isolate --json` (ticket 03).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScratchData {
    pub worktree_path: String,
    pub branch: String,
    pub lease_id: String,
    pub lease_file: String,
    pub expires_at: u64,
    pub files_hydrated: usize,
    pub hydration_method: String,
    pub bytes_shared_cow: u64,
    pub bytes_copied: u64,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleaned_up: Option<bool>,
}

/// Payload for `wt demo --json` / `wt test-drive --json` (ticket 01).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DemoData {
    pub files_count: usize,
    pub total_bytes: u64,
    pub baseline_copy_duration_ms: u64,
    pub baseline_copy_bytes: u64,
    pub wt_hydration_duration_ms: u64,
    pub speedup_ratio: f64,
    pub hydration_method: String,
    pub bytes_shared_cow: u64,
    pub bytes_copied: u64,
    pub space_savings_bytes: u64,
    pub isolation_verified: bool,
    pub cleaned_up: bool,
    pub total_duration_ms: u64,
}

/// Payload for `wt clean --json` (ticket 03 / ticket 05).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CleanData {
    pub removed_worktrees: Vec<String>,
    pub branches_removed: Vec<String>,
    pub references_released: usize,
    pub mirrors_removed: usize,
    pub reclaimed_bytes: u64,
    pub sweep_examined: usize,
    pub sweep_reclaimed: usize,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_envelope_ok_serialization() {
        let data = ScratchData {
            worktree_path: "/tmp/wt-scratch-demo".into(),
            branch: "scratch-demo".into(),
            lease_id: "demo".into(),
            lease_file: "/tmp/store/worktrees/scratch-demo.lease".into(),
            expires_at: 1900000000,
            files_hydrated: 5,
            hydration_method: "clone".into(),
            bytes_shared_cow: 512,
            bytes_copied: 0,
            duration_ms: 25,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            cleaned_up: Some(true),
        };
        let env = Envelope::ok("scratch", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize scratch json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse scratch json");
        assert_eq!(parsed["command"], "scratch");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["branch"], "scratch-demo");
        assert_eq!(parsed["data"]["command"], "cargo test");
        assert_eq!(parsed["data"]["exit_code"], 0);
        assert_eq!(parsed["data"]["cleaned_up"], true);
    }

    #[test]
    fn envelope_ok_serialization() {
        let data = CreateData {
            worktree_path: "/tmp/wt-demo".into(),
            branch: "demo".into(),
            cache_hit: true,
            duration_ms: 42,
            hydration_method: "clone".into(),
            bytes_shared_cow: 1024,
            bytes_copied: 0,
            files_hydrated: 10,
        };
        let env = Envelope::ok("create", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(!json.contains('\n'), "envelope must be single-line NDJSON");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["command"], "create");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["branch"], "demo");
        assert_eq!(parsed["data"]["cache_hit"], true);
        assert_eq!(parsed["data"]["hydration_method"], "clone");
        assert_eq!(parsed["data"]["bytes_shared_cow"], 1024);
        assert_eq!(parsed["data"]["bytes_copied"], 0);
        assert_eq!(parsed["diagnostics"], serde_json::json!([]));
    }

    #[test]
    fn envelope_error_serialization() {
        let env = Envelope::<()>::error(
            "remove",
            vec![Diagnostic::error("NOT_FOUND", "worktree not found")],
        );
        let json = serde_json::to_string(&env).expect("serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["command"], "remove");
        assert_eq!(parsed["status"], "error");
        assert!(parsed["data"].is_null());
        assert_eq!(parsed["diagnostics"][0]["code"], "NOT_FOUND");
        assert_eq!(parsed["diagnostics"][0]["level"], "error");
    }

    #[test]
    fn sweep_envelope_with_lease_metrics_serialization() {
        let data = SweepData {
            mode: "mark-sweep".into(),
            examined: 100,
            reclaimed: 5,
            mirrors_removed: Some(2),
            snapshot_dirs_removed: Some(1),
            snapshot_cap_evicted: Some(0),
            deferred_by_grace: Some(false),
            leases_examined: Some(3),
            leases_reclaimed: Some(2),
            lease_bytes_reclaimed: Some(4096),
            dry_run: None,
            unreferenced_blobs: None,
            dead_leases: None,
            reclaimed_bytes: None,
        };
        let env = Envelope::ok("sweep", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize sweep json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse sweep json");
        assert_eq!(parsed["command"], "sweep");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["leases_examined"], 3);
        assert_eq!(parsed["data"]["leases_reclaimed"], 2);
        assert_eq!(parsed["data"]["lease_bytes_reclaimed"], 4096);
    }

    #[test]
    fn doctor_envelope_ok_serialization() {
        let data = DoctorData {
            store_path: "/tmp/store".into(),
            env_vars: DoctorEnvVars {
                wt_store: Some("/tmp/store".into()),
                wt_snapshots: Some("1".into()),
                wt_snapshots_v2: None,
                wt_verify: None,
                wt_timing: None,
                wt_gc_grace: Some("15m".into()),
                wt_snapshot_cap: None,
                wt_max_snapshot_bytes: None,
                wt_hardlink: None,
                wt_no_hardlink: None,
            },
            fs_capabilities: DoctorFsCapabilities {
                apfs_clonefile: true,
                ficlone: false,
                copy_file_range: false,
            },
            store_disk_usage: StoreDuData {
                store_path: "/tmp/store".into(),
                objects_bytes: 1024,
                snapshots_bytes: 2048,
                mirrors_bytes: 512,
                refs_bytes: 0,
                caches_bytes: 256,
                total_bytes: 3840,
            },
        };
        let env = Envelope::ok("doctor", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize doctor json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse doctor json");
        assert_eq!(parsed["command"], "doctor");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["store_path"], "/tmp/store");
        assert_eq!(parsed["data"]["fs_capabilities"]["apfs_clonefile"], true);
        assert_eq!(parsed["data"]["store_disk_usage"]["total_bytes"], 3840);
    }

    #[test]
    fn store_du_envelope_ok_serialization() {
        let data = StoreDuData {
            store_path: "/tmp/store".into(),
            objects_bytes: 1000,
            snapshots_bytes: 2000,
            mirrors_bytes: 300,
            refs_bytes: 50,
            caches_bytes: 150,
            total_bytes: 3500,
        };
        let env = Envelope::ok("store du", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize store du json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse store du json");
        assert_eq!(parsed["command"], "store du");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["total_bytes"], 3500);
        assert_eq!(parsed["data"]["objects_bytes"], 1000);
    }

    #[test]
    fn demo_envelope_ok_serialization() {
        let data = DemoData {
            files_count: 10000,
            total_bytes: 2500000,
            baseline_copy_duration_ms: 200,
            baseline_copy_bytes: 2500000,
            wt_hydration_duration_ms: 15,
            speedup_ratio: 13.33,
            hydration_method: "clone".into(),
            bytes_shared_cow: 2500000,
            bytes_copied: 0,
            space_savings_bytes: 2500000,
            isolation_verified: true,
            cleaned_up: true,
            total_duration_ms: 350,
        };
        let env = Envelope::ok("demo", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize demo json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse demo json");
        assert_eq!(parsed["command"], "demo");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["files_count"], 10000);
        assert_eq!(parsed["data"]["isolation_verified"], true);
        assert_eq!(parsed["data"]["cleaned_up"], true);
    }

    #[test]
    fn list_envelope_serialization() {
        let data = ListData {
            worktrees: vec![WorktreeEntry {
                path: "/tmp/repo".into(),
                branch: "main".into(),
                head: Some("abcdef1234567890".into()),
                is_active: true,
                is_main: true,
                is_ephemeral: false,
                files_hydrated: 10,
                bytes_hydrated: 2048,
                bytes_saved: 2048,
                hydrated_dirs: Some(vec!["heavy".into()]),
                base_branch: None,
                lease: None,
                age_secs: Some(120),
            }],
            total_disk_saved: 2048,
            total_files_hydrated: 10,
        };
        let env = Envelope::ok("list", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize list json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse list json");
        assert_eq!(parsed["command"], "list");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["total_disk_saved"], 2048);
        assert_eq!(parsed["data"]["total_files_hydrated"], 10);
        assert_eq!(parsed["data"]["worktrees"][0]["branch"], "main");
        assert_eq!(parsed["data"]["worktrees"][0]["is_active"], true);
    }

    #[test]
    fn hydrate_envelope_ok_serialization() {
        let data = HydrateData {
            destination_path: "/tmp/dest".into(),
            source_path: "/tmp/src".into(),
            cache_hit: true,
            duration_ms: 12,
            hydration_method: "clone".into(),
            bytes_shared_cow: 4096,
            bytes_copied: 0,
            files_hydrated: 50,
            dirs_hydrated: vec!["node_modules".into()],
        };
        let env = Envelope::ok("hydrate", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize hydrate json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse hydrate json");
        assert_eq!(parsed["command"], "hydrate");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["destination_path"], "/tmp/dest");
        assert_eq!(parsed["data"]["source_path"], "/tmp/src");
        assert_eq!(parsed["data"]["files_hydrated"], 50);
        assert_eq!(parsed["data"]["dirs_hydrated"][0], "node_modules");
    }

    #[test]
    fn init_envelope_ok_serialization() {
        let data = InitData {
            manifest_path: "/tmp/repo/.wtinclude".into(),
            created: true,
        };
        let env = Envelope::ok("init", data, vec![]);
        let json = serde_json::to_string(&env).expect("serialize init json");
        assert!(!json.contains('\n'));
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse init json");
        assert_eq!(parsed["command"], "init");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["manifest_path"], "/tmp/repo/.wtinclude");
        assert_eq!(parsed["data"]["created"], true);
    }
}
