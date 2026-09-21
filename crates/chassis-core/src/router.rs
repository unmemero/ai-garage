use crate::attenuation::EffectiveLease;
use crate::blob::BlobStore;
use crate::error::CoreError;
use crate::policy::SecurityPolicy;
use crate::supervisor::PluginSupervisor;
use crate::wal::{WalWriter, EVENT_CAPABILITY_CHECK, EVENT_TOOL_END, EVENT_TOOL_START};
use chassis_protocol::envelope::InvokeRequest;
use chassis_protocol::error::{RpcError, CAPABILITY_NOT_FOUND, USER_REJECTED};
use chassis_protocol::Response;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// Execution mode for headless and interactive environments
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    #[default]
    Interactive,
    NonInteractiveFail,
    NonInteractiveAutoApprove,
}

/// The central Capability Router and Security Firewall
pub struct CapabilityRouter {
    plugins: Arc<RwLock<HashMap<String, PluginSupervisor>>>,
    capability_map: Arc<RwLock<HashMap<String, String>>>,
    ui_plugin_id: Arc<RwLock<Option<String>>>,
    policy: SecurityPolicy,
    workspace_root: PathBuf,
    wal_writer: Option<Arc<Mutex<WalWriter>>>,
    pub blob_store: BlobStore,
    mode: ExecutionMode,
}

impl CapabilityRouter {
    pub fn new(
        policy: SecurityPolicy,
        workspace_root: &Path,
        blob_store: BlobStore,
        mode: ExecutionMode,
        wal_writer: Option<Arc<Mutex<WalWriter>>>,
    ) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            capability_map: Arc::new(RwLock::new(HashMap::new())),
            ui_plugin_id: Arc::new(RwLock::new(None)),
            policy,
            workspace_root: workspace_root.to_path_buf(),
            wal_writer,
            blob_store,
            mode,
        }
    }

    /// Register a mounted plugin and index its offered capabilities
    pub async fn register_plugin(&self, supervisor: PluginSupervisor) {
        let plugin_id = supervisor.manifest.plugin.id.clone();
        let mut cap_map = self.capability_map.write().await;

        for offered in &supervisor.manifest.capabilities_offered {
            cap_map.insert(offered.id.clone(), plugin_id.clone());

            // Detect designated UI plugin
            if offered.id == "ui.render" || offered.id == "ui.prompt" {
                let mut ui_id = self.ui_plugin_id.write().await;
                *ui_id = Some(plugin_id.clone());
            }
        }

        let mut plugins = self.plugins.write().await;
        plugins.insert(plugin_id, supervisor);
    }

    /// Dispatch a capability invocation through the Capability Broker firewall
    pub async fn dispatch(
        &self,
        caller_id: &str,
        request: InvokeRequest,
    ) -> Result<Response, CoreError> {
        // 1. Resolve target provider plugin
        let target_plugin_id = {
            let cap_map = self.capability_map.read().await;
            match cap_map.get(&request.capability) {
                Some(id) => id.clone(),
                None => {
                    return Ok(Response::error(
                        request.call_id.clone(),
                        RpcError::new(
                            CAPABILITY_NOT_FOUND,
                            format!("No active plugin provides capability '{}'", request.capability),
                        ),
                    ));
                }
            }
        };

        let plugins = self.plugins.read().await;
        let provider = plugins.get(&target_plugin_id).ok_or_else(|| {
            CoreError::PolicyViolation(format!("Target plugin '{}' not found", target_plugin_id))
        })?;

        // 2. Capability Broker Firewall Gate: Inspect parameters against EffectiveLease
        let lease = &provider.lease;

        // Filesystem operations
        if request.capability.starts_with("tools.")
            || request.method.contains("file_")
            || request.method.contains("dir")
        {
            if let Some(path_val) = request.payload.get("path").and_then(|v| v.as_str()) {
                let is_write = request.method.contains("write")
                    || request.method.contains("create")
                    || request.method.contains("delete");

                if let Err(e) = lease.validate_path(
                    &self.workspace_root,
                    Path::new(path_val),
                    is_write,
                    &self.policy,
                ) {
                    return Ok(Response::error(
                        request.call_id.clone(),
                        RpcError::from(e),
                    ));
                }
            }
        }

        // Network operations
        if let Some(domain_val) = request.payload.get("domain").and_then(|v| v.as_str()) {
            if let Err(e) = lease.validate_network(domain_val) {
                return Ok(Response::error(
                    request.call_id.clone(),
                    RpcError::from(e),
                ));
            }
        }

        // Subprocess operations
        if let Some(binary_val) = request.payload.get("binary").and_then(|v| v.as_str()) {
            if let Err(e) = lease.validate_process(binary_val) {
                return Ok(Response::error(
                    request.call_id.clone(),
                    RpcError::from(e),
                ));
            }
        }

        // 3. Human-in-the-Loop Gatekeeper check
        let action_key = format!("{}:{}", request.capability, request.method);
        let command_param = request.payload.get("command").and_then(|v| v.as_str());

        let requires_hitl = EffectiveLease::check_hitl_requirement(
            &self.policy,
            &action_key,
            command_param,
        );

        if requires_hitl {
            match self.mode {
                ExecutionMode::NonInteractiveFail => {
                    return Ok(Response::error(
                        request.call_id.clone(),
                        RpcError::new(
                            USER_REJECTED,
                            "Action requires human approval but session is running in non-interactive mode",
                        ),
                    ));
                }
                ExecutionMode::NonInteractiveAutoApprove => {
                    // Approved automatically in disposable test environments
                }
                ExecutionMode::Interactive => {
                    // Dispatch HITL prompt to UI plugin if available
                    let ui_plugin_opt = self.ui_plugin_id.read().await.clone();
                    if let Some(ui_id) = ui_plugin_opt {
                        if let Some(ui_plugin) = plugins.get(&ui_id) {
                            let hitl_req = InvokeRequest::new(
                                format!("hitl_{}", request.call_id),
                                "ui.prompt",
                                "request_permission",
                                serde_json::json!({
                                    "action": action_key,
                                    "command": command_param,
                                    "risk": "high"
                                }),
                            );

                            let ui_resp = ui_plugin.invoke(&hitl_req).await?;
                            let approved = ui_resp
                                .result
                                .and_then(|r| r.get("approved").and_then(|a| a.as_bool()))
                                .unwrap_or(false);

                            if !approved {
                                return Ok(Response::error(
                                    request.call_id.clone(),
                                    RpcError::new(USER_REJECTED, "Action rejected by user"),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // 4. Log to Write-Ahead Log (WAL) before execution
        if let Some(wal) = &self.wal_writer {
            let mut w = wal.lock().await;
            let _ = w.append(
                EVENT_CAPABILITY_CHECK,
                serde_json::json!({
                    "caller_id": caller_id,
                    "target_plugin": target_plugin_id,
                    "capability": request.capability,
                    "method": request.method,
                    "status": "APPROVED"
                }),
            );
            let _ = w.append(
                EVENT_TOOL_START,
                serde_json::json!({
                    "call_id": request.call_id,
                    "target_plugin": target_plugin_id,
                    "method": request.method
                }),
            );
        }

        // 5. Invoke target plugin
        let response = provider.invoke(&request).await?;

        // 6. Log completion to WAL
        if let Some(wal) = &self.wal_writer {
            let mut w = wal.lock().await;
            let _ = w.append(
                EVENT_TOOL_END,
                serde_json::json!({
                    "call_id": request.call_id,
                    "has_error": response.error.is_some()
                }),
            );
        }

        Ok(response)
    }

    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }
}
