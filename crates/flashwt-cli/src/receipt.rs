use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const RECEIPT_FILENAME: &str = "flashwt-receipt.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationReceipt {
    pub operation: String,
    pub state: ReceiptState,
    pub timestamp: u64,
    pub source_root: String,
    pub dest: String,
    pub hydrated_dirs: Vec<String>,
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

impl OperationReceipt {
    pub fn new_in_progress(
        operation: impl Into<String>,
        branch: impl Into<String>,
        source_root: impl Into<String>,
        dest: impl Into<String>,
        base: Option<String>,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            operation: operation.into(),
            state: ReceiptState::InProgress,
            timestamp,
            source_root: source_root.into(),
            dest: dest.into(),
            hydrated_dirs: Vec::new(),
            branch: branch.into(),
            base,
            pid: Some(std::process::id()),
        }
    }

    pub fn complete(&mut self, hydrated_dirs: Vec<String>) {
        self.state = ReceiptState::Completed;
        self.hydrated_dirs = hydrated_dirs;
        self.timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    pub fn fail(&mut self) {
        self.state = ReceiptState::Failed;
        self.timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(json.as_bytes())?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    pub fn receipt_path(git_dir: &Path) -> PathBuf {
        git_dir.join(RECEIPT_FILENAME)
    }
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_roundtrip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let receipt_file = tmp.path().join("flashwt-receipt.json");

        let mut receipt = OperationReceipt::new_in_progress(
            "create",
            "feature-x",
            "/repo/root",
            "/repo/dest",
            Some("main".into()),
        );
        assert_eq!(receipt.state, ReceiptState::InProgress);
        assert!(receipt.hydrated_dirs.is_empty());

        receipt.save(&receipt_file).expect("save");
        let loaded = OperationReceipt::load(&receipt_file).expect("load");
        assert_eq!(loaded, receipt);

        receipt.complete(vec!["node_modules".into(), "target".into()]);
        assert_eq!(receipt.state, ReceiptState::Completed);
        assert_eq!(receipt.hydrated_dirs.len(), 2);

        receipt.save(&receipt_file).expect("save completed");
        let reloaded = OperationReceipt::load(&receipt_file).expect("load completed");
        assert_eq!(reloaded.state, ReceiptState::Completed);
        assert_eq!(reloaded.hydrated_dirs, vec!["node_modules", "target"]);
    }
}
