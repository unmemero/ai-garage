use crate::error::CoreError;
use ring::aead::{
    Aad, BoundKey, Nonce, NonceSequence, OpeningKey, SealingKey, UnboundKey, AES_256_GCM,
};
use ring::pbkdf2;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::num::NonZeroU32;
use std::path::Path;

const PBKDF2_ITERATIONS: u32 = 100_000;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

struct OneNonce(Option<Nonce>);

impl NonceSequence for OneNonce {
    fn advance(&mut self) -> Result<Nonce, ring::error::Unspecified> {
        self.0.take().ok_or(ring::error::Unspecified)
    }
}

/// Encrypted on-disk vault representation
#[derive(Debug, Serialize, Deserialize)]
struct VaultContainer {
    pub version: String,
    pub salt_hex: String,
    pub nonce_hex: String,
    pub ciphertext_hex: String,
}

/// In-memory unlocked credential store
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedVault {
    secrets: HashMap<String, String>,
}

impl EncryptedVault {
    pub fn new() -> Self {
        Self {
            secrets: HashMap::new(),
        }
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.secrets.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(|s| s.as_str())
    }

    pub fn delete(&mut self, key: &str) -> Option<String> {
        self.secrets.remove(key)
    }

    pub fn list_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.secrets.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// Derive AES-256 key from passphrase and salt using PBKDF2-HMAC-SHA256
    fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
        let mut key = [0u8; 32];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            NonZeroU32::new(PBKDF2_ITERATIONS).expect("NonZero iterations"),
            salt,
            passphrase.as_bytes(),
            &mut key,
        );
        key
    }

    /// Encrypt and save vault contents to file with strict 0600 permissions
    pub fn save_to_file(&self, path: &Path, passphrase: &str) -> Result<(), CoreError> {
        let rng = SystemRandom::new();

        let mut salt = [0u8; SALT_LEN];
        rng.fill(&mut salt)
            .map_err(|_| CoreError::VaultError("Failed to generate random salt".into()))?;

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rng.fill(&mut nonce_bytes)
            .map_err(|_| CoreError::VaultError("Failed to generate random nonce".into()))?;

        let key_bytes = Self::derive_key(passphrase, &salt);
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes)
            .map_err(|_| CoreError::VaultError("Failed to construct encryption key".into()))?;

        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| CoreError::VaultError("Invalid nonce".into()))?;

        let mut sealing_key = SealingKey::new(unbound_key, OneNonce(Some(nonce)));

        let mut in_out = serde_json::to_vec(&self.secrets)
            .map_err(|e| CoreError::VaultError(format!("Failed to serialize secrets: {e}")))?;

        sealing_key
            .seal_in_place_append_tag(Aad::empty(), &mut in_out)
            .map_err(|_| CoreError::VaultError("Encryption failed".into()))?;

        let container = VaultContainer {
            version: "1.0.0".to_string(),
            salt_hex: to_hex(&salt),
            nonce_hex: to_hex(&nonce_bytes),
            ciphertext_hex: to_hex(&in_out),
        };

        let json_str = serde_json::to_string_pretty(&container)
            .map_err(|e| CoreError::VaultError(format!("Serialization error: {e}")))?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(path, json_str)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(path, perms)?;
        }

        Ok(())
    }

    /// Load and decrypt vault contents from file
    pub fn load_from_file(path: &Path, passphrase: &str) -> Result<Self, CoreError> {
        if !path.exists() {
            return Err(CoreError::VaultError(format!(
                "Vault file does not exist at '{}'",
                path.display()
            )));
        }

        let content = fs::read_to_string(path)?;
        let container: VaultContainer = serde_json::from_str(&content)
            .map_err(|e| CoreError::VaultError(format!("Invalid vault container: {e}")))?;

        let salt = from_hex(&container.salt_hex)
            .map_err(|_| CoreError::VaultError("Invalid salt format".into()))?;
        let nonce_bytes = from_hex(&container.nonce_hex)
            .map_err(|_| CoreError::VaultError("Invalid nonce format".into()))?;
        let mut in_out = from_hex(&container.ciphertext_hex)
            .map_err(|_| CoreError::VaultError("Invalid ciphertext format".into()))?;

        if salt.len() != SALT_LEN || nonce_bytes.len() != NONCE_LEN {
            return Err(CoreError::VaultError(
                "Corrupted vault parameters (salt or nonce length)".into(),
            ));
        }

        let key_bytes = Self::derive_key(passphrase, &salt);
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes)
            .map_err(|_| CoreError::VaultError("Failed to initialize decryption key".into()))?;

        let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| CoreError::VaultError("Invalid decryption nonce".into()))?;

        let mut opening_key = OpeningKey::new(unbound_key, OneNonce(Some(nonce)));

        let plaintext = opening_key
            .open_in_place(Aad::empty(), &mut in_out)
            .map_err(|_| {
                CoreError::VaultError(
                    "Decryption failed: incorrect passphrase or corrupted vault file".into(),
                )
            })?;

        let secrets: HashMap<String, String> = serde_json::from_slice(plaintext)
            .map_err(|e| CoreError::VaultError(format!("Failed to parse decrypted secrets: {e}")))?;

        Ok(Self { secrets })
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

fn from_hex(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ())?;
        bytes.push(byte);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_vault_roundtrip_encryption() {
        let dir = tempdir().unwrap();
        let vault_file = dir.path().join("secrets.enc");

        let mut vault = EncryptedVault::new();
        vault.set("OPENAI_API_KEY", "sk-live-1234567890abcdef");
        vault.set("GITHUB_TOKEN", "ghp_securetoken9999");
        assert_eq!(vault.len(), 2);

        // Save with password
        vault
            .save_to_file(&vault_file, "correct_horse_battery_staple")
            .unwrap();
        assert!(vault_file.exists());

        // Load with correct password
        let loaded =
            EncryptedVault::load_from_file(&vault_file, "correct_horse_battery_staple").unwrap();
        assert_eq!(loaded.get("OPENAI_API_KEY"), Some("sk-live-1234567890abcdef"));
        assert_eq!(loaded.get("GITHUB_TOKEN"), Some("ghp_securetoken9999"));
        assert_eq!(loaded.list_keys(), vec!["GITHUB_TOKEN", "OPENAI_API_KEY"]);

        // Load with incorrect password must fail
        let err = EncryptedVault::load_from_file(&vault_file, "wrong_password").unwrap_err();
        assert!(matches!(err, CoreError::VaultError(_)));
        assert!(err.to_string().contains("Decryption failed"));

        // Tamper test: flip bits in ciphertext
        let content = fs::read_to_string(&vault_file).unwrap();
        let mut tampered = serde_json::from_str::<serde_json::Value>(&content).unwrap();
        let mut c_hex = tampered["ciphertext_hex"].as_str().unwrap().to_string();
        // modify one hex character
        if c_hex.starts_with("a") {
            c_hex.replace_range(0..1, "b");
        } else {
            c_hex.replace_range(0..1, "a");
        }
        tampered["ciphertext_hex"] = serde_json::Value::String(c_hex);
        fs::write(&vault_file, serde_json::to_string(&tampered).unwrap()).unwrap();

        let tamper_err =
            EncryptedVault::load_from_file(&vault_file, "correct_horse_battery_staple")
                .unwrap_err();
        assert!(tamper_err.to_string().contains("Decryption failed"));
    }
}
