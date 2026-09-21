use chassis_core::{
    BlobStore, CapabilityRouter, ExecutionMode, PluginManifest, PluginSupervisor,
    SecurityPolicy, WalReader, WalWriter,
};
use chassis_protocol::InvokeRequest;
use serde_json::json;
use std::fs;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn test_e2e_microkernel_full_lifecycle() {
    // 1. Setup isolated workspace environment
    let dir = tempdir().expect("Failed to create tempdir");
    let workspace_root = dir.path();

    let chassis_dir = workspace_root.join(".chassis");
    let sessions_dir = chassis_dir.join("sessions");
    let blobs_dir = chassis_dir.join("blobs");
    let plugins_dir = chassis_dir.join("plugins");

    fs::create_dir_all(&sessions_dir).unwrap();
    fs::create_dir_all(&blobs_dir).unwrap();
    fs::create_dir_all(&plugins_dir).unwrap();

    // 2. Initialize Security Policy (strict defaults)
    let policy_str = format!(
        r#"[policy]
version = "1.0.0"
default_action = "deny"
enforce_strict_workspaces = true

[workspace]
root = "{}"
allow_absolute_paths_outside_root = false
forbidden_patterns = ["**/.git/**", "**/.env*", "**/secrets/**"]

[network]
mode = "whitelist_only"
global_allowed_domains = ["localhost", "127.0.0.1"]
blacklisted_domains = []

[human_in_the_loop]
require_confirmation_for = ["tools.execute:dangerous_op"]
critical_command_patterns = ["rm -rf *"]
"#,
        workspace_root.display()
    );
    let policy = SecurityPolicy::from_toml(&policy_str).expect("Failed to parse policy");

    // 3. Initialize WAL Ledger
    let session_id = "ses_e2e_validation_001";
    let mut wal = WalWriter::init(session_id, &sessions_dir).expect("Failed to init WAL");
    let ev0 = wal
        .append("SESSION_START", json!({ "workspace": workspace_root.display().to_string() }))
        .expect("WAL append failed");
    assert_eq!(ev0.seq, 0);

    // 4. Initialize Blob Store and Capability Router
    let blob_store = BlobStore::new(&blobs_dir).expect("Failed to init BlobStore");
    let router = CapabilityRouter::new(
        policy.clone(),
        workspace_root,
        blob_store.clone(),
        ExecutionMode::Interactive,
        None,
    );

    // 5. Deploy Mock Plugins via shell scripts for hermetic, fast CI test execution
    let model_script = workspace_root.join("mock_model.sh");
    let model_code = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"chassis.model.test","version":"1.0.0","display_name":"Test Model","description":"Mock model","capabilities_offered":[{"id":"model.generate","version":"1.0.0","methods":["generate"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"capability/invoke"*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"model":"Meta-Llama-3.1-8B-Instruct-Q8_0.gguf","choices":[{"index":0,"message":{"role":"assistant","content":"Sovereign AI greeting received."},"finish_reason":"stop"}]}}'
      ;;
    *"kernel/ping"*)
      echo '{"jsonrpc":"2.0","id":3,"result":{"status":"healthy"}}'
      ;;
    *"kernel/shutdown"*)
      echo '{"jsonrpc":"2.0","id":4,"result":{"ready_to_exit":true}}'
      exit 0
      ;;
  esac
done
"#;
    fs::write(&model_script, model_code).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&model_script).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&model_script, p).unwrap();
    }

    let model_manifest = PluginManifest::from_toml(
        r#"
[plugin]
id = "chassis.model.test"
name = "Test Model"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./mock_model.sh"

[[capabilities_offered]]
id = "model.generate"
version = "1.0.0"
methods = ["generate"]
"#,
    )
    .unwrap();

    let tool_script = workspace_root.join("mock_tools.sh");
    let tool_code = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"chassis.tools.test","version":"1.0.0","display_name":"Test Tools","description":"Mock tools","capabilities_offered":[{"id":"tools.execute","version":"1.0.0","methods":["file_write","file_read"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"file_write"*)
      echo '{"jsonrpc":"2.0","id":2,"result":{"status":"written","bytes_written":18}}'
      ;;
    *"file_read"*)
      echo '{"jsonrpc":"2.0","id":3,"result":{"content":"sovereign_content","bytes":18}}'
      ;;
    *"kernel/shutdown"*)
      echo '{"jsonrpc":"2.0","id":4,"result":{"ready_to_exit":true}}'
      exit 0
      ;;
  esac
done
"#;
    fs::write(&tool_script, tool_code).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&tool_script).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&tool_script, p).unwrap();
    }

    let tool_manifest = PluginManifest::from_toml(
        r#"
[plugin]
id = "chassis.tools.test"
name = "Test Tools"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./mock_tools.sh"

[[capabilities_offered]]
id = "tools.execute"
version = "1.0.0"
methods = ["file_write", "file_read"]

[permissions.filesystem]
read_scopes = ["workspace"]
write_scopes = ["workspace"]
"#,
    )
    .unwrap();

    // 6. Supervise & Handshake both plugins
    let model_supervisor = PluginSupervisor::launch_and_handshake(
        model_manifest,
        &policy,
        workspace_root,
        workspace_root,
        session_id,
        Duration::from_secs(3),
    )
    .await
    .expect("Model plugin handshake failed");

    let tool_supervisor = PluginSupervisor::launch_and_handshake(
        tool_manifest,
        &policy,
        workspace_root,
        workspace_root,
        session_id,
        Duration::from_secs(3),
    )
    .await
    .expect("Tool plugin handshake failed");

    router.register_plugin(model_supervisor).await;
    router.register_plugin(tool_supervisor).await;

    wal.append("PLUGINS_REGISTERED", json!({ "count": 2 }))
        .unwrap();

    // 7. Test Model Generation invocation
    let model_invoke = InvokeRequest::new(
        "req_model_test",
        "model.generate",
        "generate",
        json!({
            "messages": [{ "role": "user", "content": "What is Chassis?" }]
        }),
    );
    let model_res = router
        .dispatch("agent_test", model_invoke)
        .await
        .expect("Model dispatch failed");
    assert!(model_res.error.is_none(), "Expected success response from model");
    let result_obj = model_res.result.expect("Missing result object");
    assert_eq!(result_obj["model"], "Meta-Llama-3.1-8B-Instruct-Q8_0.gguf");
    wal.append("MODEL_RESPONSE_RECEIVED", json!({ "status": "ok" }))
        .unwrap();

    // 8. Test Filesystem invocation (write then read)
    let write_invoke = InvokeRequest::new(
        "req_write_test",
        "tools.execute",
        "file_write",
        json!({
            "path": "test_output.txt",
            "content": "sovereign_content"
        }),
    );
    let write_res = router
        .dispatch("agent_test", write_invoke)
        .await
        .expect("Write dispatch failed");
    assert!(write_res.error.is_none());
    wal.append("FILE_WRITTEN", json!({ "path": "test_output.txt" }))
        .unwrap();

    // 9. Test Security Firewall Traversal Prevention
    let traversal_invoke = InvokeRequest::new(
        "req_traversal_test",
        "tools.execute",
        "file_read",
        json!({
            "path": "../../etc/shadow"
        }),
    );
    let traversal_res = router
        .dispatch("agent_test", traversal_invoke)
        .await
        .expect("Traversal dispatch should return RPC response");
    assert!(
        traversal_res.error.is_some(),
        "Firewall MUST reject path traversal attempt"
    );
    let err = traversal_res.error.unwrap();
    assert_eq!(err.code, chassis_protocol::POLICY_VIOLATION);
    wal.append("SECURITY_VIOLATION_BLOCKED", json!({ "code": err.code }))
        .unwrap();

    // 10. Test Blob Store offload for > 256 KB payloads
    let large_data = vec![b'X'; 300_000];
    let descriptor = blob_store
        .store(&large_data, "text/plain")
        .expect("BlobStore failed to store large payload");
    assert_eq!(descriptor.byte_length, 300_000);
    assert!(descriptor.blob.starts_with("sha256:"));

    let retrieved = blob_store
        .read(&descriptor)
        .expect("BlobStore failed to read payload");
    assert_eq!(retrieved, large_data);
    wal.append("BLOB_STORED", json!({ "blob": descriptor.blob }))
        .unwrap();

    // 11. End Session and Flush WAL
    wal.append("SESSION_END", json!({ "status": "success" }))
        .unwrap();
    let wal_path = wal.file_path().to_path_buf();
    drop(wal);

    // 12. Verify WAL Cryptographic Ledger Integrity
    let reader = WalReader::open(&wal_path).expect("Failed to open WAL reader");
    let validated_events = reader
        .validate_integrity()
        .expect("Cryptographic WAL verification failed");
    assert_eq!(validated_events.len(), 7);
    for (i, ev) in validated_events.iter().enumerate() {
        assert_eq!(ev.seq, i as u64);
        assert!(ev.hash.starts_with("sha256:"));
    }
}
