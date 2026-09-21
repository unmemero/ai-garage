use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// A pinned plugin entry in `plugins.lock.toml`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPlugin {
    pub id: String,
    pub version: String,
    #[serde(default = "default_source")]
    pub source: String,
    pub hash: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

fn default_source() -> String {
    "global".to_string()
}
fn default_true() -> bool {
    true
}

/// The workspace plugin lockfile `plugins.lock.toml`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginLockfile {
    #[serde(default = "default_lockfile_version")]
    pub version: String,
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub plugins: Vec<LockedPlugin>,
}

fn default_lockfile_version() -> String {
    "1.0.0".to_string()
}

impl PluginLockfile {
    pub fn from_toml(content: &str) -> Result<Self, CoreError> {
        let lockfile: PluginLockfile = toml::from_str(content)?;
        Ok(lockfile)
    }

    pub fn from_file(path: &Path) -> Result<Self, CoreError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content)
    }

    pub fn find_plugin(&self, id: &str) -> Option<&LockedPlugin> {
        self.plugins.iter().find(|p| p.id == id)
    }

    /// Compute SHA-256 hash of a file
    pub fn compute_file_sha256(path: &Path) -> Result<String, CoreError> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let hash = hasher.finalize();
        Ok(format!("sha256:{:x}", hash))
    }

    /// Verify an executable or bundle file against its locked SHA-256 hash
    pub fn verify_file_hash(path: &Path, expected_hash: &str) -> Result<(), CoreError> {
        let actual_hash = Self::compute_file_sha256(path)?;
        let clean_expected = expected_hash.trim();
        if clean_expected != actual_hash {
            return Err(CoreError::ChecksumMismatch {
                plugin_id: path.display().to_string(),
                expected: clean_expected.to_string(),
                actual: actual_hash,
            });
        }
        Ok(())
    }
}
