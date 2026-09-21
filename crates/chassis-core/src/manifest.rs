use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Execution runtime type for a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeType {
    #[default]
    Native,
    Script,
    Wasm,
}

/// Metadata section `[plugin]` in `plugin.toml`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub homepage: String,
}

/// Entrypoint section `[entrypoint]` in `plugin.toml`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrypointConfig {
    #[serde(default)]
    pub runtime: RuntimeType,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_passthrough: Vec<String>,
}

/// Capability exported by this plugin `[[capabilities_offered]]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferedCapability {
    pub id: String,
    pub version: String,
    pub methods: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_file: Option<PathBuf>,
}

/// Capability required by this plugin `[[capabilities_required]]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredCapability {
    pub id: String,
    #[serde(default)]
    pub optional: bool,
}

/// Network permission requests
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetworkPermissions {
    #[serde(default)]
    pub allow_outbound: bool,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub listen_ports: Vec<u16>,
}

/// Filesystem permission requests
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FilesystemPermissions {
    #[serde(default)]
    pub read_scopes: Vec<String>,
    #[serde(default)]
    pub write_scopes: Vec<String>,
}

/// Process spawning permission requests
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProcessPermissions {
    #[serde(default)]
    pub can_spawn_children: bool,
    #[serde(default)]
    pub allowed_binaries: Vec<String>,
}

/// All requested permissions declared in `[permissions]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub network: NetworkPermissions,
    #[serde(default)]
    pub filesystem: FilesystemPermissions,
    #[serde(default)]
    pub process: ProcessPermissions,
}

/// Event hook subscription `[[hooks]]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookDeclaration {
    pub event: String,
    #[serde(default = "default_hook_priority")]
    pub priority: i32,
}

fn default_hook_priority() -> i32 {
    50
}

/// Complete, validated `plugin.toml` manifest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    pub entrypoint: EntrypointConfig,
    #[serde(default)]
    pub capabilities_offered: Vec<OfferedCapability>,
    #[serde(default)]
    pub capabilities_required: Vec<RequiredCapability>,
    #[serde(default)]
    pub permissions: PermissionsConfig,
    #[serde(default)]
    pub hooks: Vec<HookDeclaration>,
}

impl PluginManifest {
    /// Parse and validate a `plugin.toml` string
    pub fn from_toml(content: &str) -> Result<Self, CoreError> {
        let manifest: PluginManifest = toml::from_str(content)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Read and parse a `plugin.toml` file from disk
    pub fn from_file(path: &Path) -> Result<Self, CoreError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content)
    }

    /// Validate manifest fields for security and correctness
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.plugin.id.trim().is_empty() {
            return Err(CoreError::ManifestParse("Plugin ID cannot be empty".into()));
        }
        if self.plugin.name.trim().is_empty() {
            return Err(CoreError::ManifestParse("Plugin name cannot be empty".into()));
        }
        if self.plugin.version.trim().is_empty() {
            return Err(CoreError::ManifestParse("Plugin version cannot be empty".into()));
        }
        if self.entrypoint.executable.as_os_str().is_empty() {
            return Err(CoreError::ManifestParse(
                "Entrypoint executable path cannot be empty".into(),
            ));
        }
        Ok(())
    }
}
