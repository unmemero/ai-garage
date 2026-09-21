use crate::error::CoreError;
use crate::manifest::PluginManifest;
use crate::policy::{NetworkMode, SecurityPolicy};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The computed, active capability lease for a running plugin
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectiveLease {
    pub plugin_id: String,
    pub enabled: bool,
    pub allowed_domains: HashSet<String>,
    pub allow_filesystem_read: bool,
    pub allow_filesystem_write: bool,
    pub can_spawn_processes: bool,
    pub allowed_binaries: HashSet<String>,
    pub secrets: HashMap<String, String>,
}

impl EffectiveLease {
    /// Compute the strict intersection between a plugin's manifest and the host policy
    pub fn compute(
        manifest: &PluginManifest,
        policy: &SecurityPolicy,
        plugin_id: &str,
    ) -> Self {
        let plugin_override = policy.plugins.get(plugin_id);
        let enabled = plugin_override.map(|o| o.enabled).unwrap_or(true);

        if !enabled {
            return Self {
                plugin_id: plugin_id.to_string(),
                enabled: false,
                ..Default::default()
            };
        }

        // 1. Network Attenuation
        let mut allowed_domains = HashSet::new();
        match policy.network.mode {
            NetworkMode::Airgapped => {
                // Zero network access regardless of plugin request
            }
            NetworkMode::Unrestricted => {
                if manifest.permissions.network.allow_outbound {
                    for domain in &manifest.permissions.network.allowed_domains {
                        if !policy.is_domain_blacklisted(domain) {
                            allowed_domains.insert(domain.to_lowercase());
                        }
                    }
                }
            }
            NetworkMode::WhitelistOnly => {
                if manifest.permissions.network.allow_outbound {
                    let candidates = if let Some(Some(override_domains)) =
                        plugin_override.map(|o| o.network_override.as_ref())
                    {
                        override_domains.clone()
                    } else {
                        manifest.permissions.network.allowed_domains.clone()
                    };

                    for domain in candidates {
                        let d_lower = domain.to_lowercase();
                        let in_global_whitelist = policy
                            .network
                            .global_allowed_domains
                            .iter()
                            .any(|g| g.to_lowercase() == d_lower);

                        if in_global_whitelist && !policy.is_domain_blacklisted(&d_lower) {
                            allowed_domains.insert(d_lower);
                        }
                    }
                }
            }
        }

        // 2. Filesystem Attenuation
        let fs_override = plugin_override.and_then(|o| o.allow_filesystem);
        let allow_fs = fs_override.unwrap_or(true);
        let allow_filesystem_read = allow_fs
            && manifest
                .permissions
                .filesystem
                .read_scopes
                .iter()
                .any(|s| s == "workspace" || s == "temp");
        let allow_filesystem_write = allow_fs
            && manifest
                .permissions
                .filesystem
                .write_scopes
                .iter()
                .any(|s| s == "workspace" || s == "temp");

        // 3. Process Spawning Attenuation
        let spawn_override = plugin_override.and_then(|o| o.allow_process_spawn);
        let can_spawn_processes = spawn_override
            .unwrap_or(manifest.permissions.process.can_spawn_children)
            && manifest.permissions.process.can_spawn_children;

        let mut allowed_binaries = HashSet::new();
        if can_spawn_processes {
            let manifest_binaries: HashSet<String> = manifest
                .permissions
                .process
                .allowed_binaries
                .iter()
                .cloned()
                .collect();

            if let Some(Some(policy_binaries)) =
                plugin_override.map(|o| o.allowed_binaries.as_ref())
            {
                let policy_set: HashSet<String> = policy_binaries.iter().cloned().collect();
                // Strict intersection: binary must be declared in both
                for b in manifest_binaries.intersection(&policy_set) {
                    allowed_binaries.insert(b.clone());
                }
            } else {
                allowed_binaries = manifest_binaries;
            }
        }

        // 4. Injected Secrets
        let secrets = plugin_override
            .map(|o| o.secrets.clone())
            .unwrap_or_default();

        Self {
            plugin_id: plugin_id.to_string(),
            enabled: true,
            allowed_domains,
            allow_filesystem_read,
            allow_filesystem_write,
            can_spawn_processes,
            allowed_binaries,
            secrets,
        }
    }

    /// Resolve "vault:<KEY>" secret references against an unlocked EncryptedVault
    pub fn resolve_secrets(&mut self, vault: &crate::vault::EncryptedVault) {
        for val in self.secrets.values_mut() {
            if let Some(vault_key) = val.strip_prefix("vault:") {
                if let Some(secret_value) = vault.get(vault_key) {
                    *val = secret_value.to_string();
                }
            }
        }
    }

    /// Validate outbound network domain access
    pub fn validate_network(&self, domain: &str) -> Result<(), CoreError> {
        if !self.enabled {
            return Err(CoreError::PolicyViolation(format!(
                "Plugin '{}' is disabled by policy",
                self.plugin_id
            )));
        }
        let d_lower = domain.to_lowercase();
        if self.allowed_domains.contains(&d_lower) {
            Ok(())
        } else {
            Err(CoreError::PolicyViolation(format!(
                "Network access to domain '{}' is not permitted for plugin '{}'",
                domain, self.plugin_id
            )))
        }
    }

    /// Validate and canonicalize filesystem access
    pub fn validate_path(
        &self,
        workspace_root: &Path,
        requested_path: &Path,
        write: bool,
        policy: &SecurityPolicy,
    ) -> Result<PathBuf, CoreError> {
        if !self.enabled {
            return Err(CoreError::PolicyViolation(format!(
                "Plugin '{}' is disabled by policy",
                self.plugin_id
            )));
        }

        if write && !self.allow_filesystem_write {
            return Err(CoreError::PolicyViolation(format!(
                "Filesystem write access is not permitted for plugin '{}'",
                self.plugin_id
            )));
        }
        if !write && !self.allow_filesystem_read {
            return Err(CoreError::PolicyViolation(format!(
                "Filesystem read access is not permitted for plugin '{}'",
                self.plugin_id
            )));
        }

        // Canonicalize workspace root
        let canonical_root = workspace_root.canonicalize().map_err(|e| {
            CoreError::PolicyViolation(format!(
                "Failed to canonicalize workspace root '{}': {}",
                workspace_root.display(),
                e
            ))
        })?;

        // Resolve target path relative to workspace root if not absolute
        let target_path = if requested_path.is_absolute() {
            requested_path.to_path_buf()
        } else {
            canonical_root.join(requested_path)
        };

        // Check if path matches any forbidden pattern
        let path_str = target_path.to_string_lossy();
        for pattern in &policy.workspace.forbidden_patterns {
            if matches_glob_pattern(pattern, &path_str) {
                return Err(CoreError::ForbiddenPath(target_path));
            }
        }

        // Canonicalize the target path if it exists to detect symlink escapes
        if target_path.exists() {
            let canonical_target = target_path.canonicalize()?;
            if !canonical_target.starts_with(&canonical_root) {
                return Err(CoreError::PathEscapeDetected {
                    path: canonical_target,
                    workspace: canonical_root,
                });
            }
            Ok(canonical_target)
        } else {
            // If the file doesn't exist yet (creating a new file), verify parent path
            let parent = target_path.parent().unwrap_or(&canonical_root);
            if parent.exists() {
                let canonical_parent = parent.canonicalize()?;
                if !canonical_parent.starts_with(&canonical_root) {
                    return Err(CoreError::PathEscapeDetected {
                        path: target_path,
                        workspace: canonical_root,
                    });
                }
                let file_name = target_path.file_name().ok_or_else(|| {
                    CoreError::PolicyViolation("Invalid target file path".into())
                })?;
                Ok(canonical_parent.join(file_name))
            } else {
                // Ensure target_path string normalized does not escape root
                let normalized = normalize_path(&target_path);
                if !normalized.starts_with(&canonical_root) {
                    return Err(CoreError::PathEscapeDetected {
                        path: normalized,
                        workspace: canonical_root,
                    });
                }
                Ok(normalized)
            }
        }
    }

    /// Validate subprocess execution permission
    pub fn validate_process(&self, binary: &str) -> Result<(), CoreError> {
        if !self.can_spawn_processes {
            return Err(CoreError::PolicyViolation(format!(
                "Process spawning is not permitted for plugin '{}'",
                self.plugin_id
            )));
        }
        if self.allowed_binaries.contains(binary) {
            Ok(())
        } else {
            Err(CoreError::PolicyViolation(format!(
                "Execution of binary '{}' is not in the allowed list for plugin '{}'",
                binary, self.plugin_id
            )))
        }
    }

    /// Check if an action or command triggers Human-in-the-Loop (HITL) confirmation
    pub fn check_hitl_requirement(
        policy: &SecurityPolicy,
        action: &str,
        command: Option<&str>,
    ) -> bool {
        // 1. Check direct action rules
        if policy
            .human_in_the_loop
            .require_confirmation_for
            .iter()
            .any(|r| r == action)
        {
            return true;
        }

        // 2. Check critical command pattern substrings
        if let Some(cmd) = command {
            let cmd_lower = cmd.to_lowercase();
            for pattern in &policy.human_in_the_loop.critical_command_patterns {
                let clean_pattern = pattern.trim_end_matches('*').trim();
                if cmd_lower.contains(clean_pattern) {
                    return true;
                }
            }
        }

        false
    }
}

/// Simple glob-style pattern matcher for forbidden patterns like `**/.git/**` or `**/.env*`
fn matches_glob_pattern(pattern: &str, path_str: &str) -> bool {
    let p = pattern.trim();
    if p.starts_with("**/") && p.ends_with("/**") {
        let keyword = &p[3..p.len() - 3];
        path_str.contains(&format!("/{}/", keyword))
            || path_str.starts_with(&format!("{}/", keyword))
            || path_str.ends_with(&format!("/{}", keyword))
    } else if p.starts_with("**/") && p.ends_with('*') {
        let keyword = &p[3..p.len() - 1];
        path_str.contains(keyword)
    } else if let Some(prefix) = p.strip_suffix("/**") {
        path_str.starts_with(prefix)
    } else {
        path_str.contains(p)
    }
}

/// Normalize path without requiring physical file existence
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(p) => out.push(p.as_os_str()),
            std::path::Component::RootDir => out.push("/"),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(c) => out.push(c),
        }
    }
    out
}
