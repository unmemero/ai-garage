use serde::{Deserialize, Serialize};

pub const METHOD_KERNEL_HANDSHAKE: &str = "kernel/handshake";
pub const METHOD_PLUGIN_ANNOUNCE: &str = "plugin/announce";
pub const METHOD_KERNEL_HANDSHAKE_ACK: &str = "kernel/handshake_ack";
pub const METHOD_KERNEL_PING: &str = "kernel/ping";
pub const METHOD_PLUGIN_PONG: &str = "plugin/pong";
pub const METHOD_KERNEL_SHUTDOWN: &str = "kernel/shutdown";
pub const METHOD_PLUGIN_LOG: &str = "plugin/log";

/// Parameters sent by Kernel to Plugin in `kernel/handshake`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeParams {
    pub protocol_version: String,
    pub kernel_version: String,
    pub session_id: String,
    pub assigned_plugin_id: String,
}

/// A capability export announced by a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDeclaration {
    pub id: String,
    pub version: String,
    pub methods: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
}

/// A capability required by a plugin
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityRequirement {
    pub id: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constraints: Option<serde_json::Value>,
}

/// An event hook subscribed to by a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookSubscription {
    pub event: String,
    #[serde(default = "default_hook_priority")]
    pub priority: i32,
}

fn default_hook_priority() -> i32 {
    100
}

/// In-memory manifest representation announced during handshake
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginAnnounceManifest {
    pub plugin_id: String,
    pub version: String,
    pub display_name: String,
    pub description: String,
    pub capabilities_offered: Vec<CapabilityDeclaration>,
    #[serde(default)]
    pub capabilities_required: Vec<CapabilityRequirement>,
    #[serde(default)]
    pub hooks_subscribed: Vec<HookSubscription>,
}

/// Result returned by plugin in response to `kernel/handshake`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginAnnounceResult {
    pub manifest: PluginAnnounceManifest,
}

/// Parameters sent by Kernel to Plugin in `kernel/handshake_ack`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeAckParams {
    pub status: String,
    pub lease_token: String,
    pub workspace_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_restrictions: Option<serde_json::Value>,
}

/// Parameters for `kernel/shutdown`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownParams {
    pub grace_timeout_ms: u64,
    pub reason: String,
}

/// Parameters for `plugin/log`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogNotification {
    pub level: String,
    pub target: String,
    pub message: String,
}
