use crate::attenuation::EffectiveLease;
use crate::error::CoreError;
use crate::lifo::LifoStack;
use crate::wal::{WalReader, WalWriter};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Request payload for delegating a sub-task to a child sub-agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentSpawnRequest {
    pub subagent_id: String,
    pub goal: String,
    pub granted_capabilities: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    60
}

/// Result returned when a sub-agent completes its delegated lifecycle
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentResult {
    pub subagent_id: String,
    pub sub_session_id: String,
    pub status: String,
    pub events_count: usize,
    pub summary: Option<String>,
}

/// Manages isolated sub-agent lifecycles, attenuated leases, and nested WALs
pub struct SubAgentManager {
    parent_session_id: String,
    sessions_dir: PathBuf,
}

impl SubAgentManager {
    pub fn new(parent_session_id: impl Into<String>, sessions_dir: &Path) -> Self {
        Self {
            parent_session_id: parent_session_id.into(),
            sessions_dir: sessions_dir.to_path_buf(),
        }
    }

    /// Validate the attenuation invariant: Capabilities(Child) subset of Capabilities(Parent)
    pub fn validate_attenuation(
        parent_capabilities: &HashSet<String>,
        child_requested: &[String],
    ) -> Result<(), CoreError> {
        for cap in child_requested {
            if !parent_capabilities.contains(cap) {
                return Err(CoreError::PolicyViolation(format!(
                    "Sub-agent capability escalation rejected: child requested '{}', but parent only has {:?}",
                    cap, parent_capabilities
                )));
            }
        }
        Ok(())
    }

    /// Spawn a child sub-agent, execute its delegated task with an isolated WAL, and teardown
    pub async fn spawn_and_execute<F, Fut>(
        &self,
        req: SubAgentSpawnRequest,
        parent_lease: &EffectiveLease,
        parent_wal: &mut WalWriter,
        task_fn: F,
    ) -> Result<SubAgentResult, CoreError>
    where
        F: FnOnce(String, PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<Option<String>, CoreError>>,
    {
        // 1. Check Attenuation: ensure child capabilities are strict subset of parent's
        let mut parent_caps = HashSet::new();
        if parent_lease.allow_filesystem_read {
            parent_caps.insert("tools.execute:file_read".to_string());
            parent_caps.insert("tools.execute:list_dir".to_string());
        }
        if parent_lease.allow_filesystem_write {
            parent_caps.insert("tools.execute:file_write".to_string());
        }
        for bin in &parent_lease.allowed_binaries {
            parent_caps.insert(format!("tools.process:{}", bin));
        }
        if !parent_lease.allowed_domains.is_empty() {
            parent_caps.insert("network:outbound".to_string());
        }

        Self::validate_attenuation(&parent_caps, &req.granted_capabilities)?;

        // 2. Initialize Sub-Session and Nested WAL
        let sub_session_id = format!("{}.sub.{}", self.parent_session_id, req.subagent_id);
        let mut sub_wal = WalWriter::init(&sub_session_id, &self.sessions_dir)?;
        let sub_wal_path = sub_wal.file_path().to_path_buf();

        parent_wal.append(
            "SUBAGENT_SPAWNED",
            json!({
                "subagent_id": req.subagent_id,
                "sub_session_id": sub_session_id,
                "goal": req.goal,
                "granted_capabilities": req.granted_capabilities,
            }),
        )?;

        sub_wal.append(
            "SUBAGENT_START",
            json!({
                "parent_session_id": self.parent_session_id,
                "subagent_id": req.subagent_id,
                "goal": req.goal,
                "granted_capabilities": req.granted_capabilities,
            }),
        )?;

        // 3. Isolated LIFO cleanup guard
        let mut lifo = LifoStack::new();
        let subagent_name = req.subagent_id.clone();
        lifo.push(format!("subagent_teardown_{}", subagent_name), move || {
            tracing::info!(subagent = %subagent_name, "Reclaiming child subagent resources in LIFO order");
        });

        // 4. Execute delegated task within timeout window
        let timeout_dur = Duration::from_secs(req.timeout_secs);
        let execution_result = tokio::time::timeout(
            timeout_dur,
            task_fn(sub_session_id.clone(), sub_wal_path.clone()),
        )
        .await;

        let task_summary = match execution_result {
            Ok(Ok(summary)) => {
                sub_wal.append(
                    "SUBAGENT_SUCCESS",
                    json!({ "summary": summary.as_deref().unwrap_or("completed") }),
                )?;
                summary
            }
            Ok(Err(e)) => {
                sub_wal.append("SUBAGENT_FAILED", json!({ "error": e.to_string() }))?;
                return Err(e);
            }
            Err(_) => {
                sub_wal.append(
                    "SUBAGENT_TIMEOUT",
                    json!({ "timeout_secs": req.timeout_secs }),
                )?;
                return Err(CoreError::PolicyViolation(format!(
                    "Sub-agent '{}' exceeded deadline of {}s",
                    req.subagent_id, req.timeout_secs
                )));
            }
        };

        sub_wal.append("SUBAGENT_END", json!({ "status": "clean_exit" }))?;
        drop(sub_wal);

        // 5. Verify Nested WAL Cryptographic Integrity
        let reader = WalReader::open(&sub_wal_path)?;
        let verified_events = reader.validate_integrity()?;

        parent_wal.append(
            "SUBAGENT_RETURNED",
            json!({
                "subagent_id": req.subagent_id,
                "status": "success",
                "nested_wal_events": verified_events.len(),
                "summary": task_summary,
            }),
        )?;

        // LIFO drops deterministically on scope exit
        drop(lifo);

        Ok(SubAgentResult {
            subagent_id: req.subagent_id,
            sub_session_id,
            status: "success".to_string(),
            events_count: verified_events.len(),
            summary: task_summary,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_subagent_lease_attenuation_validation() {
        let mut parent_caps = HashSet::new();
        parent_caps.insert("tools.execute:file_read".to_string());
        parent_caps.insert("tools.execute:list_dir".to_string());

        // 1. Valid attenuated subset
        let valid_child = vec!["tools.execute:file_read".to_string()];
        assert!(SubAgentManager::validate_attenuation(&parent_caps, &valid_child).is_ok());

        // 2. Escalation attempt: child wants file_write which parent doesn't have
        let invalid_child = vec![
            "tools.execute:file_read".to_string(),
            "tools.execute:file_write".to_string(),
        ];
        let err = SubAgentManager::validate_attenuation(&parent_caps, &invalid_child).unwrap_err();
        assert!(matches!(err, CoreError::PolicyViolation(_)));
        assert!(err.to_string().contains("escalation rejected"));
    }

    #[tokio::test]
    async fn test_subagent_lifecycle_and_nested_wal() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path();

        let mut parent_wal = WalWriter::init("ses_parent_001", sessions_dir).unwrap();
        parent_wal
            .append("SESSION_START", json!({ "role": "parent" }))
            .unwrap();

        let parent_lease = EffectiveLease {
            allow_filesystem_read: true,
            ..Default::default()
        };

        let manager = SubAgentManager::new("ses_parent_001", sessions_dir);
        let req = SubAgentSpawnRequest {
            subagent_id: "researcher_01".to_string(),
            goal: "Scan headers".to_string(),
            granted_capabilities: vec!["tools.execute:file_read".to_string()],
            timeout_secs: 5,
        };

        let result = manager
            .spawn_and_execute(req, &parent_lease, &mut parent_wal, |_sub_id, _wal_path| async {
                // Simulate subagent work
                Ok(Some("Found 3 header files".to_string()))
            })
            .await
            .unwrap();

        assert_eq!(result.status, "success");
        assert_eq!(result.events_count, 3); // START, SUCCESS, END
        assert_eq!(result.summary.unwrap(), "Found 3 header files");

        // Verify parent WAL records
        parent_wal
            .append("SESSION_END", json!({ "status": "done" }))
            .unwrap();
        let parent_reader = WalReader::open(parent_wal.file_path()).unwrap();
        let parent_events = parent_reader.validate_integrity().unwrap();
        assert_eq!(parent_events.len(), 4); // START, SUBAGENT_SPAWNED, SUBAGENT_RETURNED, SESSION_END

        // Verify nested WAL file exists and is cryptographically integral
        let sub_wal_file = sessions_dir.join("ses_parent_001.sub.researcher_01.wal.jsonl");
        assert!(sub_wal_file.exists());
        let sub_reader = WalReader::open(&sub_wal_file).unwrap();
        let sub_events = sub_reader.validate_integrity().unwrap();
        assert_eq!(sub_events.len(), 3);
        assert_eq!(sub_events[0].event_type, "SUBAGENT_START");
        assert_eq!(sub_events[1].event_type, "SUBAGENT_SUCCESS");
        assert_eq!(sub_events[2].event_type, "SUBAGENT_END");
    }
}
