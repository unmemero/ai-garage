use crate::error::CoreError;
use chassis_protocol::envelope::BlobRef;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Threshold for large payload spillover: 256 KB
pub const BLOB_SPILLOVER_THRESHOLD_BYTES: usize = 256 * 1024;

/// Content-addressed scratch blob store
#[derive(Debug, Clone)]
pub struct BlobStore {
    blob_dir: PathBuf,
}

impl BlobStore {
    pub fn new(scratch_dir: &Path) -> Result<Self, CoreError> {
        let blob_dir = scratch_dir.join("blobs");
        std::fs::create_dir_all(&blob_dir)?;
        Ok(Self { blob_dir })
    }

    /// Store binary data if it exceeds the spillover threshold
    pub fn store(&self, data: &[u8], mime_type: impl Into<String>) -> Result<BlobRef, CoreError> {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash_hex = format!("{:x}", hasher.finalize());
        let blob_id = format!("sha256:{}", hash_hex);

        let file_path = self.blob_dir.join(&hash_hex);
        std::fs::write(&file_path, data)?;

        Ok(BlobRef::new(blob_id, mime_type, data.len() as u64))
    }

    /// Read binary data by its content-addressed identifier
    pub fn read(&self, blob_ref: &BlobRef) -> Result<Vec<u8>, CoreError> {
        let hash_hex = blob_ref
            .blob
            .strip_prefix("sha256:")
            .unwrap_or(&blob_ref.blob);

        let file_path = self.blob_dir.join(hash_hex);
        if !file_path.exists() {
            return Err(CoreError::PolicyViolation(format!(
                "Blob payload not found on disk: '{}'",
                blob_ref.blob
            )));
        }

        let data = std::fs::read(&file_path)?;

        // Verify SHA-256 integrity upon read
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let computed_hex = format!("{:x}", hasher.finalize());
        if computed_hex != hash_hex {
            return Err(CoreError::PolicyViolation(format!(
                "Blob checksum mismatch: expected {}, got sha256:{}",
                blob_ref.blob, computed_hex
            )));
        }

        Ok(data)
    }

    pub fn blob_dir(&self) -> &Path {
        &self.blob_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_blob_store_and_read() {
        let dir = tempdir().unwrap();
        let store = BlobStore::new(dir.path()).unwrap();

        let data = b"Hello from large payload spillover test!";
        let blob_ref = store.store(data, "text/plain").unwrap();

        assert!(blob_ref.blob.starts_with("sha256:"));
        assert_eq!(blob_ref.byte_length, data.len() as u64);
        assert_eq!(blob_ref.mime_type, "text/plain");

        let retrieved = store.read(&blob_ref).unwrap();
        assert_eq!(retrieved, data);
    }
}
