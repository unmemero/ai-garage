//! Chassis Core Runtime Library
//!
//! Provides manifest parsing, host security policy evaluation,
//! capability lease attenuation, lockfile validation, and process supervision.

pub mod attenuation;
pub mod blob;
pub mod error;
pub mod lifo;
pub mod lockfile;
pub mod manifest;
pub mod policy;
pub mod process;
pub mod router;
pub mod supervisor;
pub mod subagent;
pub mod vault;
pub mod wal;

pub use attenuation::EffectiveLease;
pub use blob::BlobStore;
pub use error::CoreError;
pub use lifo::{CleanupGuard, LifoStack};
pub use lockfile::{LockedPlugin, PluginLockfile};
pub use manifest::PluginManifest;
pub use policy::SecurityPolicy;
pub use process::{collect_descendants, kill_process_tree, ProcessHandle};
pub use router::{CapabilityRouter, ExecutionMode};
pub use subagent::{SubAgentManager, SubAgentResult, SubAgentSpawnRequest};
pub use supervisor::PluginSupervisor;
pub use vault::EncryptedVault;
pub use wal::{WalEvent, WalReader, WalWriter};

/// Mark the host microkernel process as a child subreaper on Linux.
///
/// If any descendant processes become orphaned (by double-forking or setsid),
/// the Linux kernel will reparent them directly to Chassis instead of PID 1,
/// allowing the microkernel to track, kill, and reap all orphaned processes.
#[cfg(target_os = "linux")]
pub fn init_subreaper() {
    const PR_SET_CHILD_SUBREAPER: libc::c_int = 36;
    unsafe {
        libc::prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn init_subreaper() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::tempdir;

    const SAMPLE_MANIFEST: &str = r#"
[plugin]
id = "chassis.model.openai"
name = "OpenAI Model Provider"
version = "1.2.0"
description = "Streaming inference for OpenAI"

[entrypoint]
runtime = "native"
executable = "./bin/openai_adapter"
args = ["--verbose"]
env_passthrough = []

[[capabilities_offered]]
id = "model.generate"
version = "1.0.0"
methods = ["generate", "embed"]

[[capabilities_required]]
id = "context.workspace"
optional = false

[permissions.network]
allow_outbound = true
allowed_domains = ["api.openai.com", "untrusted.tracker.com"]

[permissions.filesystem]
read_scopes = ["workspace"]
write_scopes = []

[permissions.process]
can_spawn_children = true
allowed_binaries = ["git", "curl"]

[[hooks]]
event = "session.start"
priority = 100
"#;

    const SAMPLE_POLICY: &str = r#"
[policy]
version = "1.0.0"
default_action = "deny"
enforce_strict_workspaces = true

[workspace]
root = "/tmp/fake_workspace"
allow_absolute_paths_outside_root = false
forbidden_patterns = ["**/.git/**", "**/.env*", "**/secrets/**"]

[network]
mode = "whitelist_only"
global_allowed_domains = ["api.openai.com", "api.anthropic.com"]
blacklisted_domains = ["*.tracker.com"]

[human_in_the_loop]
require_confirmation_for = ["tools.execute:file_delete", "tools.execute:git_push"]
critical_command_patterns = ["rm -rf *", "git push --force*"]

[plugins."chassis.model.openai"]
enabled = true
allow_filesystem = true
allow_process_spawn = true
allowed_binaries = ["git"]
[plugins."chassis.model.openai".secrets]
OPENAI_API_KEY = "vault:OPENAI_API_KEY"
"#;

    #[test]
    fn test_manifest_parsing_and_validation() {
        let manifest = PluginManifest::from_toml(SAMPLE_MANIFEST).expect("Failed to parse manifest");
        assert_eq!(manifest.plugin.id, "chassis.model.openai");
        assert_eq!(manifest.plugin.version, "1.2.0");
        assert_eq!(manifest.capabilities_offered.len(), 1);
        assert_eq!(manifest.capabilities_offered[0].id, "model.generate");
        assert!(manifest.permissions.network.allow_outbound);
        assert_eq!(manifest.permissions.network.allowed_domains.len(), 2);
    }

    #[test]
    fn test_policy_parsing_and_defaults() {
        let policy = SecurityPolicy::from_toml(SAMPLE_POLICY).expect("Failed to parse policy");
        assert_eq!(policy.policy.default_action, "deny");
        assert!(policy.is_domain_blacklisted("untrusted.tracker.com"));
        assert!(!policy.is_domain_blacklisted("api.openai.com"));
    }

    #[test]
    fn test_policy_attenuation_intersection() {
        let manifest = PluginManifest::from_toml(SAMPLE_MANIFEST).unwrap();
        let policy = SecurityPolicy::from_toml(SAMPLE_POLICY).unwrap();

        let lease = EffectiveLease::compute(&manifest, &policy, "chassis.model.openai");
        assert!(lease.enabled);

        // Network: manifest had "api.openai.com" and "untrusted.tracker.com".
        // Policy whitelists "api.openai.com" and blacklists "*.tracker.com".
        // Result: ONLY "api.openai.com" must survive.
        assert!(lease.allowed_domains.contains("api.openai.com"));
        assert!(!lease.allowed_domains.contains("untrusted.tracker.com"));
        assert_eq!(lease.allowed_domains.len(), 1);

        // Process spawning: manifest had ["git", "curl"]. Policy override had ["git"].
        // Strict intersection: ONLY ["git"] must survive.
        assert!(lease.allowed_binaries.contains("git"));
        assert!(!lease.allowed_binaries.contains("curl"));
        assert_eq!(lease.allowed_binaries.len(), 1);

        // Secrets: must be securely bound from policy
        assert_eq!(
            lease.secrets.get("OPENAI_API_KEY").unwrap(),
            "vault:OPENAI_API_KEY"
        );

        // Network validation check
        assert!(lease.validate_network("api.openai.com").is_ok());
        assert!(lease.validate_network("untrusted.tracker.com").is_err());
        assert!(lease.validate_network("evil.com").is_err());
    }

    #[test]
    fn test_path_traversal_and_forbidden_checks() {
        let dir = tempdir().expect("Failed to create tempdir");
        let workspace_root = dir.path();

        // Create a safe subfolder and a safe file
        let src_dir = workspace_root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let safe_file = src_dir.join("main.rs");
        File::create(&safe_file).unwrap().write_all(b"fn main() {}").unwrap();

        // Create a forbidden file (.env)
        let env_file = workspace_root.join(".env");
        File::create(&env_file).unwrap().write_all(b"SECRET=123").unwrap();

        let manifest = PluginManifest::from_toml(SAMPLE_MANIFEST).unwrap();
        let mut policy = SecurityPolicy::from_toml(SAMPLE_POLICY).unwrap();
        policy.workspace.root = workspace_root.to_path_buf();

        let lease = EffectiveLease::compute(&manifest, &policy, "chassis.model.openai");

        // 1. Reading safe file must succeed
        let validated = lease.validate_path(workspace_root, Path::new("src/main.rs"), false, &policy);
        assert!(validated.is_ok(), "Reading safe file must be allowed");

        // 2. Reading forbidden .env must fail with ForbiddenPath
        let forbidden = lease.validate_path(workspace_root, Path::new(".env"), false, &policy);
        assert!(
            matches!(forbidden, Err(CoreError::ForbiddenPath(_))),
            "Reading .env must be rejected"
        );

        // 3. Path traversal escaping root must fail with PathEscapeDetected
        let escape = lease.validate_path(
            workspace_root,
            Path::new("../../etc/passwd"),
            false,
            &policy,
        );
        assert!(
            matches!(escape, Err(CoreError::PathEscapeDetected { .. })),
            "Path traversal outside root must be rejected"
        );
    }

    #[test]
    fn test_lockfile_hash_verification() {
        let dir = tempdir().unwrap();
        let test_file = dir.path().join("test_bin");
        File::create(&test_file).unwrap().write_all(b"binary content here").unwrap();

        let correct_hash = PluginLockfile::compute_file_sha256(&test_file).unwrap();
        assert!(correct_hash.starts_with("sha256:"));

        // Verification with matching hash succeeds
        assert!(PluginLockfile::verify_file_hash(&test_file, &correct_hash).is_ok());

        // Verification with modified/corrupted hash fails
        let fake_hash = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let err = PluginLockfile::verify_file_hash(&test_file, fake_hash).unwrap_err();
        assert!(matches!(err, CoreError::ChecksumMismatch { .. }));
    }

    #[test]
    fn test_hitl_rule_triggers() {
        let policy = SecurityPolicy::from_toml(SAMPLE_POLICY).unwrap();

        // 1. Direct action match
        assert!(EffectiveLease::check_hitl_requirement(
            &policy,
            "tools.execute:git_push",
            None
        ));

        // 2. Safe action
        assert!(!EffectiveLease::check_hitl_requirement(
            &policy,
            "tools.execute:file_read",
            None
        ));

        // 3. Critical command pattern (rm -rf)
        assert!(EffectiveLease::check_hitl_requirement(
            &policy,
            "tools.execute:shell_execute",
            Some("rm -rf target/")
        ));

        // 4. Critical command pattern (git push --force)
        assert!(EffectiveLease::check_hitl_requirement(
            &policy,
            "tools.execute:shell_execute",
            Some("git push --force origin main")
        ));

        // 5. Safe command
        assert!(!EffectiveLease::check_hitl_requirement(
            &policy,
            "tools.execute:shell_execute",
            Some("cargo check")
        ));
    }

    #[test]
    fn test_wal_append_and_integrity_verification() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path();

        let mut writer = WalWriter::init("ses_test_001", sessions_dir).unwrap();
        assert_eq!(writer.current_seq(), 0);

        // Append 3 events
        let e0 = writer
            .append("SESSION_START", serde_json::json!({ "root": "/test" }))
            .unwrap();
        assert_eq!(e0.seq, 0);
        assert_eq!(
            e0.prev_hash,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert!(e0.hash.starts_with("sha256:"));

        let e1 = writer
            .append("USER_INPUT", serde_json::json!({ "prompt": "build app" }))
            .unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e1.prev_hash, e0.hash);

        let e2 = writer
            .append("TOOL_START", serde_json::json!({ "tool": "file_read" }))
            .unwrap();
        assert_eq!(e2.seq, 2);
        assert_eq!(e2.prev_hash, e1.hash);

        // Read and verify integrity
        let reader = WalReader::open(writer.file_path()).unwrap();
        let events = reader.validate_integrity().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
        assert_eq!(events[2].seq, 2);
    }

    #[test]
    fn test_wal_tamper_detection() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path();

        let mut writer = WalWriter::init("ses_tamper_001", sessions_dir).unwrap();
        writer
            .append("SESSION_START", serde_json::json!({ "v": 1 }))
            .unwrap();
        writer
            .append("USER_INPUT", serde_json::json!({ "text": "hello" }))
            .unwrap();
        writer
            .append("SESSION_END", serde_json::json!({ "status": 0 }))
            .unwrap();

        let wal_path = writer.file_path().to_path_buf();
        drop(writer);

        // Tamper with the file: replace "hello" with "hacked" in the second line
        let content = std::fs::read_to_string(&wal_path).unwrap();
        let tampered_content = content.replace("hello", "hacked");
        std::fs::write(&wal_path, tampered_content).unwrap();

        // Validation MUST detect the broken cryptographic hash
        let reader = WalReader::open(&wal_path).unwrap();
        let err = reader.validate_integrity().unwrap_err();
        assert!(
            matches!(err, CoreError::WalIntegrityViolation { .. }),
            "Expected WalIntegrityViolation on tampered file, got {:?}",
            err
        );
    }

    #[test]
    fn test_wal_resume_existing_session() {
        let dir = tempdir().unwrap();
        let sessions_dir = dir.path();

        // Session writer 1 appends 2 events
        {
            let mut writer1 = WalWriter::init("ses_resume_001", sessions_dir).unwrap();
            writer1
                .append("SESSION_START", serde_json::json!({ "start": true }))
                .unwrap();
            writer1
                .append("STEP_1", serde_json::json!({ "step": 1 }))
                .unwrap();
            assert_eq!(writer1.current_seq(), 2);
        }

        // Session writer 2 reopens the same session ID
        {
            let mut writer2 = WalWriter::init("ses_resume_001", sessions_dir).unwrap();
            assert_eq!(
                writer2.current_seq(),
                2,
                "Should resume at sequence 2 after existing entries"
            );

            let e2 = writer2
                .append("STEP_2", serde_json::json!({ "step": 2 }))
                .unwrap();
            assert_eq!(e2.seq, 2);

            let reader = WalReader::open(writer2.file_path()).unwrap();
            let events = reader.validate_integrity().unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[2].event_type, "STEP_2");
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_supervisor_handshake_and_lifecycle() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let plugin_dir = dir.path();
        let script_path = plugin_dir.join("mock_plugin.sh");

        // Write mock plugin script
        let script_content = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"mock.plugin","version":"1.0.0","display_name":"Mock Plugin","description":"Test","capabilities_offered":[{"id":"test.op","version":"1.0.0","methods":["do_work"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"kernel/ping"*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"status":"healthy"}}'
      ;;
    *"kernel/shutdown"*)
      echo '{"jsonrpc":"2.0","id":3,"result":{"ready_to_exit":true}}'
      exit 0
      ;;
  esac
done
"#;
        std::fs::write(&script_path, script_content).unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let manifest_content = r#"
[plugin]
id = "mock.plugin"
name = "Mock Plugin"
version = "1.0.0"
description = "Test"

[entrypoint]
runtime = "script"
executable = "./mock_plugin.sh"
args = []
env_passthrough = []
"#;
        let manifest = PluginManifest::from_toml(manifest_content).unwrap();
        let mut policy = SecurityPolicy::default();
        policy.workspace.root = plugin_dir.to_path_buf();

        // 1. Launch & Handshake
        let mut supervisor = PluginSupervisor::launch_and_handshake(
            manifest,
            &policy,
            plugin_dir,
            plugin_dir,
            "ses_mock_001",
            Duration::from_secs(3),
        )
        .await
        .expect("Handshake should succeed");

        // 2. Ping / Pong
        let is_healthy = supervisor
            .ping(Duration::from_secs(2))
            .await
            .expect("Ping should succeed");
        assert!(is_healthy, "Plugin should respond healthy to ping");

        // 3. Graceful Shutdown
        supervisor
            .shutdown(Duration::from_millis(500))
            .await
            .expect("Shutdown should complete");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_capability_router_security_firewall() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let workspace_root = dir.path();
        let script_path = workspace_root.join("tool_plugin.sh");

        // Write mock tool plugin script
        let script_content = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"mock.tool","version":"1.0.0","display_name":"Tool","description":"T","capabilities_offered":[{"id":"tools.execute","version":"1.0.0","methods":["file_read"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"capability/invoke"*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"content":"file contents here"}}'
      ;;
    *"kernel/shutdown"*)
      echo '{"jsonrpc":"2.0","id":3,"result":{"ready_to_exit":true}}'
      exit 0
      ;;
  esac
done
"#;
        std::fs::write(&script_path, script_content).unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let manifest_content = r#"
[plugin]
id = "mock.tool"
name = "Mock Tool"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./tool_plugin.sh"

[[capabilities_offered]]
id = "tools.execute"
version = "1.0.0"
methods = ["file_read"]

[permissions.filesystem]
read_scopes = ["workspace"]
"#;
        let manifest = PluginManifest::from_toml(manifest_content).unwrap();
        let mut policy = SecurityPolicy::default();
        policy.workspace.root = workspace_root.to_path_buf();

        let supervisor = PluginSupervisor::launch_and_handshake(
            manifest,
            &policy,
            workspace_root,
            workspace_root,
            "ses_router_001",
            Duration::from_secs(3),
        )
        .await
        .unwrap();

        let blob_store = BlobStore::new(workspace_root).unwrap();
        let router = CapabilityRouter::new(
            policy,
            workspace_root,
            blob_store,
            ExecutionMode::Interactive,
            None,
        );

        router.register_plugin(supervisor).await;

        // 1. Path traversal attempt must be blocked by the router firewall
        let malicious_req = chassis_protocol::InvokeRequest::new(
            "call_hack",
            "tools.execute",
            "file_read",
            serde_json::json!({ "path": "../../etc/passwd" }),
        );

        let resp = router.dispatch("agent_01", malicious_req).await.unwrap();
        assert!(resp.error.is_some(), "Malicious path must return an error");
        let err = resp.error.unwrap();
        assert_eq!(err.code, chassis_protocol::POLICY_VIOLATION);
        assert!(err.message.contains("escapes allowed workspace root"));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_capability_router_hitl_non_interactive() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let workspace_root = dir.path();
        let script_path = workspace_root.join("shell_plugin.sh");

        let script_content = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"mock.shell","version":"1.0.0","display_name":"Shell","description":"S","capabilities_offered":[{"id":"tools.shell","version":"1.0.0","methods":["execute"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"kernel/shutdown"*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"ready_to_exit":true}}'
      exit 0
      ;;
  esac
done
"#;
        std::fs::write(&script_path, script_content).unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let manifest_content = r#"
[plugin]
id = "mock.shell"
name = "Mock Shell"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./shell_plugin.sh"

[[capabilities_offered]]
id = "tools.shell"
version = "1.0.0"
methods = ["execute"]

[permissions.process]
can_spawn_children = true
allowed_binaries = ["rm"]
"#;
        let manifest = PluginManifest::from_toml(manifest_content).unwrap();
        let mut policy = SecurityPolicy::default();
        policy.workspace.root = workspace_root.to_path_buf();
        policy.human_in_the_loop.critical_command_patterns = vec!["rm -rf *".into()];

        let supervisor = PluginSupervisor::launch_and_handshake(
            manifest,
            &policy,
            workspace_root,
            workspace_root,
            "ses_hitl_001",
            Duration::from_secs(3),
        )
        .await
        .unwrap();

        let blob_store = BlobStore::new(workspace_root).unwrap();
        // Set NonInteractiveFail mode
        let router = CapabilityRouter::new(
            policy,
            workspace_root,
            blob_store,
            ExecutionMode::NonInteractiveFail,
            None,
        );

        router.register_plugin(supervisor).await;

        // Command matches critical command pattern rm -rf *
        let destructive_req = chassis_protocol::InvokeRequest::new(
            "call_del",
            "tools.shell",
            "execute",
            serde_json::json!({ "command": "rm -rf /" }),
        );

        let resp = router.dispatch("agent_01", destructive_req).await.unwrap();
        assert!(resp.error.is_some(), "Destructive action must return an error in non-interactive mode");
        let err = resp.error.unwrap();
        assert_eq!(err.code, chassis_protocol::USER_REJECTED);
        assert!(err.message.contains("non-interactive mode"));
    }
}
