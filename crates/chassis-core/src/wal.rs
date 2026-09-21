use crate::error::CoreError;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Canonical event types for the Chassis Write-Ahead Log
pub const EVENT_SESSION_START: &str = "SESSION_START";
pub const EVENT_USER_INPUT: &str = "USER_INPUT";
pub const EVENT_PROMPT_RESOLVED: &str = "PROMPT_RESOLVED";
pub const EVENT_LLM_REQUEST: &str = "LLM_REQUEST";
pub const EVENT_LLM_RESPONSE: &str = "LLM_RESPONSE";
pub const EVENT_CAPABILITY_CHECK: &str = "CAPABILITY_CHECK";
pub const EVENT_HITL_PROMPT: &str = "HITL_PROMPT";
pub const EVENT_HITL_DECISION: &str = "HITL_DECISION";
pub const EVENT_TOOL_START: &str = "TOOL_START";
pub const EVENT_TOOL_END: &str = "TOOL_END";
pub const EVENT_CHECKPOINT: &str = "CHECKPOINT";
pub const EVENT_SESSION_END: &str = "SESSION_END";

/// An immutable, hash-chained session event in the Write-Ahead Log
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalEvent {
    /// Monotonically increasing sequence number (0, 1, 2, ...)
    pub seq: u64,
    /// SHA-256 hash of the immediately preceding event
    pub prev_hash: String,
    /// Cryptographic SHA-256 hash of this event
    pub hash: String,
    /// ISO 8601 UTC timestamp with nanoseconds
    pub timestamp_utc: String,
    /// Session identifier this event belongs to
    pub session_id: String,
    /// Event category / type
    pub event_type: String,
    /// Arbitrary domain payload
    pub payload: serde_json::Value,
}

impl WalEvent {
    /// Compute the deterministic SHA-256 hash for an event envelope
    pub fn compute_hash(
        prev_hash: &str,
        seq: u64,
        timestamp_utc: &str,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> Result<String, CoreError> {
        let payload_canonical = serde_json::to_string(payload).map_err(|e| {
            CoreError::PolicyViolation(format!("Failed to serialize WAL payload: {}", e))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(prev_hash.as_bytes());
        hasher.update(b":");
        hasher.update(seq.to_string().as_bytes());
        hasher.update(b":");
        hasher.update(timestamp_utc.as_bytes());
        hasher.update(b":");
        hasher.update(event_type.as_bytes());
        hasher.update(b":");
        hasher.update(payload_canonical.as_bytes());

        let hash_bytes = hasher.finalize();
        Ok(format!("sha256:{:x}", hash_bytes))
    }
}

/// Append-Only Write-Ahead Log Writer with synchronous `fsync` guarantees
pub struct WalWriter {
    file: File,
    file_path: PathBuf,
    session_id: String,
    current_seq: u64,
    last_hash: String,
}

impl WalWriter {
    /// Initialize or open a session WAL file inside the given sessions directory
    pub fn init(session_id: impl Into<String>, sessions_dir: &Path) -> Result<Self, CoreError> {
        let sid = session_id.into();
        std::fs::create_dir_all(sessions_dir)?;

        let file_path = sessions_dir.join(format!("{}.wal.jsonl", sid));
        let exists = file_path.exists();

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)?;

        let mut current_seq = 0;
        let mut last_hash = GENESIS_HASH.to_string();

        if exists {
            // Read existing entries to recover state
            let reader = WalReader::open(&file_path)?;
            let events = reader.validate_integrity()?;
            if let Some(last_event) = events.last() {
                current_seq = last_event.seq + 1;
                last_hash = last_event.hash.clone();
            }
        }

        // Update active_session.link symlink if on unix
        #[cfg(unix)]
        {
            let link_path = sessions_dir.join("active_session.link");
            let _ = std::fs::remove_file(&link_path);
            let target_filename_str = format!("{}.wal.jsonl", sid);
            let target_filename = Path::new(&target_filename_str);
            let _ = std::os::unix::fs::symlink(target_filename, &link_path);
        }

        Ok(Self {
            file,
            file_path,
            session_id: sid,
            current_seq,
            last_hash,
        })
    }

    /// Append an event to the WAL, immediately syncing to disk via `fsync`
    pub fn append(
        &mut self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<WalEvent, CoreError> {
        let timestamp = Utc::now().to_rfc3339();
        let seq = self.current_seq;
        let prev_hash = self.last_hash.clone();

        let hash = WalEvent::compute_hash(&prev_hash, seq, &timestamp, event_type, &payload)?;

        let event = WalEvent {
            seq,
            prev_hash,
            hash: hash.clone(),
            timestamp_utc: timestamp,
            session_id: self.session_id.clone(),
            event_type: event_type.to_string(),
            payload,
        };

        // Serialize as single-line NDJSON
        let mut line = serde_json::to_string(&event).map_err(|e| {
            CoreError::PolicyViolation(format!("Failed to serialize WAL event: {}", e))
        })?;
        line.push('\n');

        // Write and synchronously flush to disk
        self.file.write_all(line.as_bytes())?;
        self.file.sync_all()?;

        self.current_seq += 1;
        self.last_hash = hash;

        Ok(event)
    }

    pub fn current_seq(&self) -> u64 {
        self.current_seq
    }

    pub fn last_hash(&self) -> &str {
        &self.last_hash
    }

    pub fn file_path(&self) -> &Path {
        &self.file_path
    }
}

/// WAL Reader and Cryptographic Integrity Verifier
pub struct WalReader {
    file_path: PathBuf,
}

impl WalReader {
    pub fn open(file_path: &Path) -> Result<Self, CoreError> {
        if !file_path.exists() {
            return Err(CoreError::PolicyViolation(format!(
                "WAL file does not exist: {}",
                file_path.display()
            )));
        }
        Ok(Self {
            file_path: file_path.to_path_buf(),
        })
    }

    /// Read all events and verify the cryptographic SHA-256 hash chain and monotonic sequence
    pub fn validate_integrity(&self) -> Result<Vec<WalEvent>, CoreError> {
        let file = File::open(&self.file_path)?;
        let reader = BufReader::new(file);

        let mut events = Vec::new();
        let mut expected_seq = 0;
        let mut expected_prev_hash = GENESIS_HASH.to_string();

        for (line_idx, line_result) in reader.lines().enumerate() {
            let line = line_result?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let event: WalEvent = serde_json::from_str(trimmed).map_err(|e| {
                CoreError::WalIntegrityViolation {
                    seq: expected_seq,
                    reason: format!("Failed to parse line {} as WalEvent: {}", line_idx + 1, e),
                }
            })?;

            // 1. Monotonic Sequence Check
            if event.seq != expected_seq {
                return Err(CoreError::WalIntegrityViolation {
                    seq: event.seq,
                    reason: format!(
                        "Sequence mismatch: expected seq {}, found seq {}",
                        expected_seq, event.seq
                    ),
                });
            }

            // 2. Previous Hash Link Check
            if event.prev_hash != expected_prev_hash {
                return Err(CoreError::WalIntegrityViolation {
                    seq: event.seq,
                    reason: format!(
                        "Hash chain broken: expected prev_hash {}, found {}",
                        expected_prev_hash, event.prev_hash
                    ),
                });
            }

            // 3. Cryptographic Signature Recomputation
            let recomputed_hash = WalEvent::compute_hash(
                &event.prev_hash,
                event.seq,
                &event.timestamp_utc,
                &event.event_type,
                &event.payload,
            )?;

            if event.hash != recomputed_hash {
                return Err(CoreError::WalIntegrityViolation {
                    seq: event.seq,
                    reason: format!(
                        "Cryptographic hash invalid at seq {}: expected {}, recomputed {}",
                        event.seq, event.hash, recomputed_hash
                    ),
                });
            }

            expected_prev_hash = event.hash.clone();
            expected_seq += 1;
            events.push(event);
        }

        Ok(events)
    }
}
