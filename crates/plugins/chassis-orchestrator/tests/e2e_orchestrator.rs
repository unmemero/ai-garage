use chassis_core::{
    BlobStore, CapabilityRouter, ExecutionMode, PluginManifest, PluginSupervisor, SecurityPolicy,
    WalWriter,
};
use chassis_protocol::InvokeRequest;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::main]
#[test]
async fn test_orchestrator_react_goal_loop_e2e() {
    let temp = tempdir().expect("Failed to create tempdir");
    let workspace = temp.path().to_path_buf();
    let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let sessions_dir = workspace.join(".chassis/sessions");
    let blobs_dir = workspace.join(".chassis/blobs");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&blobs_dir).unwrap();

    let mut policy = SecurityPolicy::default();
    policy.workspace.root = workspace.clone();

    let blob_store = BlobStore::new(&blobs_dir).unwrap();
    let wal = WalWriter::init("test_orch_session", &sessions_dir).unwrap();
    let wal_arc = Arc::new(tokio::sync::Mutex::new(wal));

    let router = Arc::new(CapabilityRouter::new(
        policy.clone(),
        &workspace,
        blob_store,
        ExecutionMode::Interactive,
        Some(Arc::clone(&wal_arc)),
    ));
    let reverse_tx = router.create_reverse_channel();

    // 1. Boot chassis-tools-filesystem
    let fs_dir = root_dir.join("crates/plugins/chassis-tools-filesystem");
    let fs_manifest_str = std::fs::read_to_string(fs_dir.join("plugin.toml")).unwrap();
    let fs_manifest = PluginManifest::from_toml(&fs_manifest_str).unwrap();
    let fs_sup = PluginSupervisor::launch_and_handshake_full(
        fs_manifest,
        &policy,
        &workspace,
        &fs_dir,
        "test_orch_session",
        Duration::from_secs(5),
        None,
        Some(reverse_tx.clone()),
    )
    .await
    .expect("Failed to launch filesystem tools");
    router.register_plugin(fs_sup).await;

    // 2. Boot chassis-model-local
    let model_dir = root_dir.join("crates/plugins/chassis-model-local");
    let model_manifest_str = std::fs::read_to_string(model_dir.join("plugin.toml")).unwrap();
    let model_manifest = PluginManifest::from_toml(&model_manifest_str).unwrap();
    let model_sup = PluginSupervisor::launch_and_handshake_full(
        model_manifest,
        &policy,
        &workspace,
        &model_dir,
        "test_orch_session",
        Duration::from_secs(5),
        None,
        Some(reverse_tx.clone()),
    )
    .await
    .expect("Failed to launch model adapter");
    router.register_plugin(model_sup).await;

    // 3. Boot chassis-orchestrator
    let orch_dir = root_dir.join("crates/plugins/chassis-orchestrator");
    let orch_manifest_str = std::fs::read_to_string(orch_dir.join("plugin.toml")).unwrap();
    let orch_manifest = PluginManifest::from_toml(&orch_manifest_str).unwrap();
    let orch_sup = PluginSupervisor::launch_and_handshake_full(
        orch_manifest,
        &policy,
        &workspace,
        &orch_dir,
        "test_orch_session",
        Duration::from_secs(5),
        None,
        Some(reverse_tx.clone()),
    )
    .await
    .expect("Failed to launch orchestrator plugin");
    router.register_plugin(orch_sup).await;

    // 4. Dispatch autonomous goal to the orchestrator
    let goal_req = InvokeRequest::new(
        "test_goal_001",
        "agent.orchestrate",
        "run_goal",
        json!({
            "goal": "Inspect workspace files and report project status",
            "max_steps": 3
        }),
    );

    let goal_resp = router
        .dispatch("cli", goal_req)
        .await
        .expect("Dispatch failed");

    assert!(
        goal_resp.error.is_none(),
        "Orchestrator returned error: {:?}",
        goal_resp.error
    );

    let res = goal_resp.result.expect("Expected result payload");
    assert_eq!(res.get("status").and_then(|s| s.as_str()), Some("completed"));
    let steps_count = res.get("steps").and_then(|s| s.as_u64()).unwrap_or(0);
    assert!(steps_count >= 1, "Expected at least 1 step");

    let final_ans = res.get("final_answer").and_then(|a| a.as_str()).unwrap_or("");
    assert!(!final_ans.is_empty(), "Final answer must not be empty");

    let history = res.get("history").and_then(|h| h.as_array()).expect("History must be array");
    assert!(!history.is_empty(), "History must contain trajectory steps");

    // Verify first step executed tool call
    let step1 = &history[0];
    assert!(step1.get("thought").is_some());
    assert!(step1.get("action").is_some());
    assert!(step1.get("observation").is_some());

    // 5. Dispatch chat_turn
    let chat_req = InvokeRequest::new(
        "test_chat_001",
        "agent.orchestrate",
        "chat_turn",
        json!({
            "message": "Hello sovereign agent"
        }),
    );

    let chat_resp = router
        .dispatch("cli", chat_req)
        .await
        .expect("Chat turn dispatch failed");
    assert!(chat_resp.error.is_none());
    let chat_res = chat_resp.result.expect("Expected chat result");
    assert!(chat_res.get("response").is_some());

    // 6. Test capability attenuation enforcement:
    // If a plugin tries to dispatch a capability it DID NOT declare in [[capabilities_required]],
    // the microkernel blocks it!
    let unauthorized_req = InvokeRequest::new(
        "test_unauth_001",
        "chassis.tools.filesystem", // Attempt to call filesystem directly as a capability without declaring it
        "list_dir",
        json!({ "path": "." }),
    );

    let unauth_resp = router
        .dispatch("chassis.agent.orchestrator", unauthorized_req)
        .await
        .expect("Dispatch returned error");

    assert!(
        unauth_resp.error.is_some(),
        "Microkernel must reject undeclared capability invocation"
    );
}
