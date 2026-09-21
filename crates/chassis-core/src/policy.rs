use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Network operating mode
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    Airgapped,
    #[default]
    WhitelistOnly,
    Unrestricted,
}

/// Global policy metadata `[policy]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyMeta {
    #[serde(default = "default_policy_version")]
    pub version: String,
    #[serde(default = "default_deny_action")]
    pub default_action: String,
    #[serde(default = "default_true")]
    pub enforce_strict_workspaces: bool,
}

fn default_policy_version() -> String {
    "1.0.0".to_string()
}
fn default_deny_action() -> String {
    "deny".to_string()
}
fn default_true() -> bool {
    true
}

impl Default for PolicyMeta {
    fn default() -> Self {
        Self {
            version: default_policy_version(),
            default_action: default_deny_action(),
            enforce_strict_workspaces: true,
        }
    }
}

/// Workspace boundary policy `[workspace]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePolicy {
    pub root: PathBuf,
    #[serde(default)]
    pub allow_absolute_paths_outside_root: bool,
    #[serde(default = "default_forbidden_patterns")]
    pub forbidden_patterns: Vec<String>,
}

fn default_forbidden_patterns() -> Vec<String> {
    vec![
        "**/.git/**".into(),
        "**/.env*".into(),
        "**/id_rsa*".into(),
        "**/*.pem".into(),
        "**/secrets/**".into(),
        "/etc/**".into(),
        "/var/**".into(),
        "/proc/**".into(),
    ]
}

impl Default for WorkspacePolicy {
    fn default() -> Self {
        Self {
            root: PathBuf::from("."),
            allow_absolute_paths_outside_root: false,
            forbidden_patterns: default_forbidden_patterns(),
        }
    }
}

/// Network firewall policy `[network]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NetworkPolicy {
    #[serde(default)]
    pub mode: NetworkMode,
    #[serde(default)]
    pub global_allowed_domains: Vec<String>,
    #[serde(default)]
    pub blacklisted_domains: Vec<String>,
}

/// Human-in-the-Loop policy `[human_in_the_loop]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HitlPolicy {
    #[serde(default)]
    pub require_confirmation_for: Vec<String>,
    #[serde(default)]
    pub critical_command_patterns: Vec<String>,
}

/// Per-plugin overrides `[plugins.<id>]`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PluginPolicyOverride {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub network_override: Option<Vec<String>>,
    #[serde(default)]
    pub allow_filesystem: Option<bool>,
    #[serde(default)]
    pub allow_process_spawn: Option<bool>,
    #[serde(default)]
    pub allowed_binaries: Option<Vec<String>>,
    #[serde(default)]
    pub secrets: HashMap<String, String>,
}

/// Complete Host Security Policy (`security_policy.toml`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SecurityPolicy {
    #[serde(default)]
    pub policy: PolicyMeta,
    pub workspace: WorkspacePolicy,
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub human_in_the_loop: HitlPolicy,
    #[serde(default)]
    pub plugins: HashMap<String, PluginPolicyOverride>,
}

impl SecurityPolicy {
    /// Parse and validate a `security_policy.toml` string
    pub fn from_toml(content: &str) -> Result<Self, CoreError> {
        let policy: SecurityPolicy = toml::from_str(content)?;
        Ok(policy)
    }

    /// Read and parse a `security_policy.toml` file from disk
    pub fn from_file(path: &Path) -> Result<Self, CoreError> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content)
    }

    /// Check if a domain is explicitly blacklisted
    pub fn is_domain_blacklisted(&self, domain: &str) -> bool {
        let d = domain.to_lowercase();
        self.network.blacklisted_domains.iter().any(|pattern| {
            if let Some(suffix) = pattern.strip_prefix("*.") {
                d.ends_with(suffix) || d == suffix
            } else {
                d == pattern.to_lowercase()
            }
        })
    }
}
