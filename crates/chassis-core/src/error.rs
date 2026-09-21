use chassis_protocol::error::{RpcError, POLICY_VIOLATION};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Manifest parse error: {0}")]
    ManifestParse(String),

    #[error("Security policy parse error: {0}")]
    PolicyParse(String),

    #[error("Lockfile parse error: {0}")]
    LockfileParse(String),

    #[error("Policy violation: {0}")]
    PolicyViolation(String),

    #[error("Path escape detected: {path} escapes workspace root {workspace}")]
    PathEscapeDetected {
        path: PathBuf,
        workspace: PathBuf,
    },

    #[error("Forbidden path pattern matched: {0}")]
    ForbiddenPath(PathBuf),

    #[error("Integrity check failed: checksum mismatch for plugin '{plugin_id}' (expected {expected}, found {actual})")]
    ChecksumMismatch {
        plugin_id: String,
        expected: String,
        actual: String,
    },

    #[error("WAL integrity check failed at seq {seq}: {reason}")]
    WalIntegrityViolation {
        seq: u64,
        reason: String,
    },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Vault error: {0}")]
    VaultError(String),
}

impl From<CoreError> for RpcError {
    fn from(err: CoreError) -> Self {
        match err {
            CoreError::PolicyViolation(msg) => RpcError::new(POLICY_VIOLATION, msg),
            CoreError::PathEscapeDetected { path, workspace } => RpcError::new(
                POLICY_VIOLATION,
                format!(
                    "Path '{}' escapes allowed workspace root '{}'",
                    path.display(),
                    workspace.display()
                ),
            ),
            CoreError::ForbiddenPath(path) => RpcError::new(
                POLICY_VIOLATION,
                format!("Access to forbidden path '{}' is denied by policy", path.display()),
            ),
            _ => RpcError::internal_error(err.to_string()),
        }
    }
}
