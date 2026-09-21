use crate::attenuation::EffectiveLease;
use crate::error::CoreError;
use crate::lifo::LifoStack;
use crate::manifest::PluginManifest;
use crate::policy::SecurityPolicy;
use crate::process::ProcessHandle;
use chassis_protocol::envelope::InvokeRequest;
use chassis_protocol::lifecycle::{
    HandshakeAckParams, HandshakeParams, PluginAnnounceResult, METHOD_KERNEL_HANDSHAKE,
    METHOD_KERNEL_HANDSHAKE_ACK, METHOD_KERNEL_PING, METHOD_PLUGIN_PONG,
};
use chassis_protocol::Response;
use std::path::Path;
use std::time::Duration;

/// Plugin supervisor managing lifecycle, handshakes, and LIFO teardown
pub struct PluginSupervisor {
    pub manifest: PluginManifest,
    pub lease: EffectiveLease,
    pub process: ProcessHandle,
    pub lifo: LifoStack,
    session_id: String,
}

impl PluginSupervisor {
    /// Launch and negotiate capability handshake with a plugin
    pub async fn launch_and_handshake(
        manifest: PluginManifest,
        policy: &SecurityPolicy,
        workspace_root: &Path,
        plugin_dir: &Path,
        session_id: impl Into<String>,
        handshake_timeout: Duration,
    ) -> Result<Self, CoreError> {
        Self::launch_and_handshake_with_vault(
            manifest,
            policy,
            workspace_root,
            plugin_dir,
            session_id,
            handshake_timeout,
            None,
        )
        .await
    }

    /// Launch and negotiate capability handshake with selective vault secret injection
    pub async fn launch_and_handshake_with_vault(
        manifest: PluginManifest,
        policy: &SecurityPolicy,
        workspace_root: &Path,
        plugin_dir: &Path,
        session_id: impl Into<String>,
        handshake_timeout: Duration,
        vault: Option<&crate::vault::EncryptedVault>,
    ) -> Result<Self, CoreError> {
        let sid = session_id.into();
        let mut lease = EffectiveLease::compute(&manifest, policy, &manifest.plugin.id);
        if let Some(v) = vault {
            lease.resolve_secrets(v);
        }

        if !lease.enabled {
            return Err(CoreError::PolicyViolation(format!(
                "Plugin '{}' is disabled by policy",
                manifest.plugin.id
            )));
        }

        let process = ProcessHandle::spawn(&manifest, &lease, workspace_root, plugin_dir)?;
        let mut lifo = LifoStack::new();

        // Handshake Stage 1: Send kernel/handshake request
        let handshake_params = HandshakeParams {
            protocol_version: "1.0.0".to_string(),
            kernel_version: "0.1.0".to_string(),
            session_id: sid.clone(),
            assigned_plugin_id: manifest.plugin.id.clone(),
        };

        let response = tokio::time::timeout(
            handshake_timeout,
            process.send_request(
                METHOD_KERNEL_HANDSHAKE,
                Some(serde_json::to_value(&handshake_params).map_err(|e| {
                    CoreError::PolicyViolation(format!("Failed to serialize handshake params: {}", e))
                })?),
            ),
        )
        .await
        .map_err(|_| {
            CoreError::PolicyViolation(format!(
                "Handshake timed out after {:?} for plugin '{}'",
                handshake_timeout, manifest.plugin.id
            ))
        })??;

        if let Some(err) = response.error {
            return Err(CoreError::PolicyViolation(format!(
                "Plugin '{}' rejected handshake: {}",
                manifest.plugin.id, err.message
            )));
        }

        let result_value = response.result.ok_or_else(|| {
            CoreError::PolicyViolation("Handshake response missing result payload".into())
        })?;

        let _announce: PluginAnnounceResult = serde_json::from_value(result_value).map_err(|e| {
            CoreError::PolicyViolation(format!(
                "Invalid plugin/announce payload from '{}': {}",
                manifest.plugin.id, e
            ))
        })?;

        // Handshake Stage 2: Send kernel/handshake_ack notification
        let ack_params = HandshakeAckParams {
            status: "mounted".to_string(),
            lease_token: format!("lease_{}", manifest.plugin.id),
            workspace_root: workspace_root.display().to_string(),
            sandbox_restrictions: None,
        };

        process
            .send_notification(
                METHOD_KERNEL_HANDSHAKE_ACK,
                Some(serde_json::to_value(&ack_params).map_err(|e| {
                    CoreError::PolicyViolation(format!("Failed to serialize handshake_ack: {}", e))
                })?),
            )
            .await?;

        // Register LIFO cleanup guard
        let pid_opt = process.pid();
        lifo.push(
            format!("plugin_teardown_{}", manifest.plugin.id),
            move || {
                #[cfg(unix)]
                if let Some(pid) = pid_opt {
                    crate::process::kill_process_tree(pid);
                }
            },
        );

        Ok(Self {
            manifest,
            lease,
            process,
            lifo,
            session_id: sid,
        })
    }

    /// Check plugin liveness via ping/pong
    pub async fn ping(&self, timeout: Duration) -> Result<bool, CoreError> {
        let res = tokio::time::timeout(
            timeout,
            self.process.send_request(
                METHOD_KERNEL_PING,
                Some(serde_json::json!({ "timestamp": chrono::Utc::now().timestamp() })),
            ),
        )
        .await;

        match res {
            Ok(Ok(response)) => {
                if let Some(result) = response.result {
                    Ok(result.get("status").and_then(|s| s.as_str()) == Some("healthy")
                        || result.get("method").and_then(|m| m.as_str()) == Some(METHOD_PLUGIN_PONG))
                } else {
                    Ok(false)
                }
            }
            _ => Ok(false),
        }
    }

    /// Forward a capability invocation to this plugin process
    pub async fn invoke(&self, request: &InvokeRequest) -> Result<Response, CoreError> {
        let params = serde_json::to_value(request).map_err(|e| {
            CoreError::PolicyViolation(format!("Failed to serialize invoke request: {}", e))
        })?;

        self.process
            .send_request(chassis_protocol::METHOD_CAPABILITY_INVOKE, Some(params))
            .await
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Graceful shutdown
    pub async fn shutdown(&mut self, timeout: Duration) -> Result<(), CoreError> {
        self.process.shutdown(timeout).await
    }
}
