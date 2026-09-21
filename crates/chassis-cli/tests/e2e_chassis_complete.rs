use chassis_core::{
    EffectiveLease, PluginManifest, SecurityPolicy, SubAgentManager, SubAgentSpawnRequest,
    WalReader, WalWriter,
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

#[tokio::test]
async fn test_complete_chassis_e2e() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let cli_bin = repo_root.join("target/debug/chassis-cli");

    // Ensure binaries are compiled
    assert!(
        cli_bin.exists(),
        "chassis-cli binary must exist at: {}",
        cli_bin.display()
    );

    // 1. Setup isolated workspace
    let dir = tempdir().expect("Failed to create tempdir");
    let ws = dir.path();

    // -------------------------------------------------------------
    // STAGE 1: chassis init
    // -------------------------------------------------------------
    let init_out = Command::new(&cli_bin)
        .arg("init")
        .arg(ws)
        .output()
        .expect("Failed to execute chassis init");
    assert!(init_out.status.success(), "chassis init failed");
    let init_stdout = String::from_utf8_lossy(&init_out.stdout);
    assert!(init_stdout.contains("Workspace initialized successfully"));

    let chassis_dir = ws.join(".chassis");
    assert!(chassis_dir.join("sessions").is_dir());
    assert!(chassis_dir.join("blobs").is_dir());
    assert!(chassis_dir.join("keychains").is_dir());
    assert!(chassis_dir.join("plugins").is_dir());
    assert!(chassis_dir.join("security_policy.toml").is_file());
    assert!(chassis_dir.join("plugins.lock.toml").is_file());

    // -------------------------------------------------------------
    // STAGE 2: chassis secret set & list
    // -------------------------------------------------------------
    let secret_set_out = Command::new(&cli_bin)
        .arg("secret")
        .arg("set")
        .arg("OPENAI_API_KEY")
        .arg("sk-e2e-secret-key-12345")
        .arg("--workspace")
        .arg(ws)
        .env("CHASSIS_VAULT_PASSWORD", "master_e2e_passphrase")
        .output()
        .expect("Failed to execute chassis secret set");
    assert!(secret_set_out.status.success());
    assert!(chassis_dir.join("keychains/secrets.enc").exists());

    let secret_list_out = Command::new(&cli_bin)
        .arg("secret")
        .arg("list")
        .arg("--workspace")
        .arg(ws)
        .env("CHASSIS_VAULT_PASSWORD", "master_e2e_passphrase")
        .output()
        .expect("Failed to execute chassis secret list");
    assert!(secret_list_out.status.success());
    let secret_list_stdout = String::from_utf8_lossy(&secret_list_out.stdout);
    assert!(secret_list_stdout.contains("OPENAI_API_KEY"));
    assert!(secret_list_stdout.contains("sha256:"));
    // Crucial zero-leakage check: plaintext secret must NEVER appear in output
    assert!(
        !secret_list_stdout.contains("sk-e2e-secret-key-12345"),
        "Plaintext secret leaked in list output!"
    );

    // -------------------------------------------------------------
    // STAGE 3: Install Reference Plugins into workspace .chassis/plugins/
    // -------------------------------------------------------------
    let model_plugin_dir = chassis_dir.join("plugins/chassis-model-local");
    let fs_plugin_dir = chassis_dir.join("plugins/chassis-tools-filesystem");
    fs::create_dir_all(&model_plugin_dir).unwrap();
    fs::create_dir_all(&fs_plugin_dir).unwrap();

    let model_src_manifest =
        repo_root.join("crates/plugins/chassis-model-local/plugin.toml");
    let fs_src_manifest =
        repo_root.join("crates/plugins/chassis-tools-filesystem/plugin.toml");

    fs::copy(&model_src_manifest, model_plugin_dir.join("plugin.toml")).unwrap();
    fs::copy(&fs_src_manifest, fs_plugin_dir.join("plugin.toml")).unwrap();

    // Copy executable binaries into plugin folders
    fs::copy(
        repo_root.join("target/debug/chassis-model-local"),
        model_plugin_dir.join("chassis-model-local"),
    )
    .unwrap();
    fs::copy(
        repo_root.join("target/debug/chassis-tools-filesystem"),
        fs_plugin_dir.join("chassis-tools-filesystem"),
    )
    .unwrap();

    // -------------------------------------------------------------
    // STAGE 4: chassis run (Full microkernel boot & capability loop)
    // -------------------------------------------------------------
    let run_out = Command::new(&cli_bin)
        .arg("run")
        .arg("--workspace")
        .arg(ws)
        .arg("--non-interactive")
        .env("CHASSIS_VAULT_PASSWORD", "master_e2e_passphrase")
        .output()
        .expect("Failed to execute chassis run");

    assert!(run_out.status.success(), "chassis run failed: {:?}", run_out);
    let run_stdout = String::from_utf8_lossy(&run_out.stdout);
    assert!(run_stdout.contains("Unlocked encrypted vault"));
    assert!(run_stdout.contains("Booting plugin: chassis.model.local"));
    assert!(run_stdout.contains("Booting plugin: chassis.tools.filesystem"));
    assert!(run_stdout.contains("2 plugins online and verified"));
    assert!(run_stdout.contains("Model generation response received"));
    assert!(run_stdout.contains("Filesystem tools response received"));
    assert!(run_stdout.contains("Microkernel session finished successfully"));

    // -------------------------------------------------------------
    // STAGE 5: Cryptographic WAL Replay & Anti-Tamper Verification
    // -------------------------------------------------------------
    let sessions_dir = chassis_dir.join("sessions");
    let session_entries: Vec<_> = fs::read_dir(&sessions_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    assert_eq!(session_entries.len(), 1, "Exactly one session WAL expected");
    let wal_path = session_entries[0].path();

    let replay_out = Command::new(&cli_bin)
        .arg("replay")
        .arg(&wal_path)
        .arg("--workspace")
        .arg(ws)
        .output()
        .expect("Failed to execute chassis replay");
    assert!(replay_out.status.success());
    let replay_stdout = String::from_utf8_lossy(&replay_out.stdout);
    assert!(replay_stdout.contains("Cryptographic verification: PASSED"));
    assert!(replay_stdout.contains("100% untampered SHA-256 forward-hash chain"));

    // Tampering test: modify a single byte in the recorded WAL
    let original_wal = fs::read_to_string(&wal_path).unwrap();
    let tampered_wal = original_wal.replacen("model.generate", "hacked.generate", 1);
    fs::write(&wal_path, tampered_wal).unwrap();

    let tampered_replay_out = Command::new(&cli_bin)
        .arg("replay")
        .arg(&wal_path)
        .arg("--workspace")
        .arg(ws)
        .output()
        .expect("Failed to execute chassis replay on tampered file");
    // Tamper detection must FAIL the command
    assert!(
        !tampered_replay_out.status.success(),
        "Replay should fail on tampered WAL ledger!"
    );

    // Restore original WAL
    fs::write(&wal_path, original_wal).unwrap();

    // -------------------------------------------------------------
    // STAGE 6: Hierarchical Sub-Agent Delegation & Attenuation
    // -------------------------------------------------------------
    let mut parent_wal = WalWriter::init("ses_e2e_parent", &sessions_dir).unwrap();
    parent_wal
        .append("SESSION_START", json!({ "agent": "e2e_root" }))
        .unwrap();

    let parent_lease = EffectiveLease {
        allow_filesystem_read: true,
        ..Default::default()
    };

    let subagent_mgr = SubAgentManager::new("ses_e2e_parent", &sessions_dir);

    // Child requesting unpermitted capability (privilege escalation) must fail
    let escalation_req = SubAgentSpawnRequest {
        subagent_id: "escalator".to_string(),
        goal: "Break sandbox".to_string(),
        granted_capabilities: vec![
            "tools.execute:file_read".to_string(),
            "tools.execute:file_write".to_string(), // Parent only has read!
        ],
        timeout_secs: 5,
    };
    let escalation_err = subagent_mgr
        .spawn_and_execute(escalation_req, &parent_lease, &mut parent_wal, |_id, _path| async {
            Ok(None)
        })
        .await
        .unwrap_err();
    assert!(escalation_err.to_string().contains("escalation rejected"));

    // Valid child sub-agent execution
    let valid_req = SubAgentSpawnRequest {
        subagent_id: "auditor".to_string(),
        goal: "Audit codebase".to_string(),
        granted_capabilities: vec!["tools.execute:file_read".to_string()],
        timeout_secs: 5,
    };
    let sub_result = subagent_mgr
        .spawn_and_execute(valid_req, &parent_lease, &mut parent_wal, |_id, _path| async {
            Ok(Some("Audit completed with 0 findings".to_string()))
        })
        .await
        .unwrap();

    assert_eq!(sub_result.status, "success");
    assert_eq!(
        sub_result.summary.unwrap(),
        "Audit completed with 0 findings"
    );

    // Verify nested sub-agent WAL was created and cryptographically verified
    let nested_wal_file = sessions_dir.join("ses_e2e_parent.sub.auditor.wal.jsonl");
    assert!(nested_wal_file.exists());
    let nested_reader = WalReader::open(&nested_wal_file).unwrap();
    let nested_events = nested_reader.validate_integrity().unwrap();
    assert_eq!(nested_events.len(), 3);

    // -------------------------------------------------------------
    // STAGE 7: Security Policy Firewall Interception
    // -------------------------------------------------------------
    let policy_path = chassis_dir.join("security_policy.toml");
    let policy_str = fs::read_to_string(&policy_path).unwrap();
    let policy = SecurityPolicy::from_toml(&policy_str).unwrap();
    let fs_manifest_str = fs::read_to_string(&fs_src_manifest).unwrap();
    let fs_manifest = PluginManifest::from_toml(&fs_manifest_str).unwrap();
    let lease = EffectiveLease::compute(&fs_manifest, &policy, "chassis.tools.filesystem");

    // 1. Path traversal escaping workspace root must be caught
    let escape_err = lease.validate_path(ws, std::path::Path::new("../../etc/shadow"), false, &policy);
    assert!(escape_err.is_err());

    // 2. Accessing forbidden .env must be caught
    let env_err = lease.validate_path(ws, std::path::Path::new(".env"), false, &policy);
    assert!(env_err.is_err());
}
