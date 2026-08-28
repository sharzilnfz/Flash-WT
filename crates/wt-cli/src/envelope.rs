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

    #[allow(dead_code)]
    pub fn info(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            level: Some("info".into()),
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
}

/// Payload for `wt scrub --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScrubData {
    pub dry_run: bool,
    pub scanned: u64,
    pub corrupt: Vec<String>,
    pub deleted: u64,
}

/// Payload for `wt store migrate --json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrateData {
    pub gc_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purged_legacy_refs: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
