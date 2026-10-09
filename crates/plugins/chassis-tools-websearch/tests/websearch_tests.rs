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

#[tokio::test]
async fn test_websearch_plugin_e2e_through_microkernel() {
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
    let wal = WalWriter::init("test_search_session", &sessions_dir).unwrap();
    let wal_arc = Arc::new(tokio::sync::Mutex::new(wal));

    let router = Arc::new(CapabilityRouter::new(
        policy.clone(),
        &workspace,
        blob_store,
        ExecutionMode::Interactive,
        Some(Arc::clone(&wal_arc)),
    ));
    let reverse_tx = router.create_reverse_channel();

    // Boot chassis-tools-websearch
    let search_dir = root_dir.join("crates/plugins/chassis-tools-websearch");
    let search_manifest_str = std::fs::read_to_string(search_dir.join("plugin.toml")).unwrap();
    let search_manifest = PluginManifest::from_toml(&search_manifest_str).unwrap();

    let search_sup = PluginSupervisor::launch_and_handshake_full(
        search_manifest,
        &policy,
        &workspace,
        &search_dir,
        "test_search_session",
        Duration::from_secs(5),
        None,
        Some(reverse_tx),
    )
    .await
    .expect("Failed to launch web search tool plugin");

    router.register_plugin(search_sup).await;

    // 1. Dispatch web_search capability
    let search_req = InvokeRequest::new(
        "search_req_001",
        "tools.search",
        "web_search",
        json!({
            "query": "Rust programming language",
            "max_results": 3
        }),
    );

    let search_resp = router
        .dispatch("cli", search_req)
        .await
        .expect("Dispatch failed");

    assert!(search_resp.error.is_none(), "Search error: {:?}", search_resp.error);
    let result = search_resp.result.expect("Expected result payload");
    let results_arr = result.get("results").and_then(|r| r.as_array()).expect("results array");
    assert!(!results_arr.is_empty(), "Should return at least 1 search result");

    let first = &results_arr[0];
    assert!(first.get("title").is_some());
    assert!(first.get("snippet").is_some());
    assert!(first.get("url").is_some());
    assert!(first.get("source").is_some());

    // 2. Dispatch fetch_page capability for an allowed domain
    let fetch_req = InvokeRequest::new(
        "fetch_req_001",
        "tools.search",
        "fetch_page",
        json!({
            "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        }),
    );

    let fetch_resp = router
        .dispatch("cli", fetch_req)
        .await
        .expect("Dispatch failed");
    assert!(fetch_resp.error.is_none(), "Fetch error: {:?}", fetch_resp.error);

    // 3. Test Network Policy Firewall Block:
    // If a request tries to access a domain not in allowed_domains,
    // the microkernel rejects it at the capability broker firewall!
    let blocked_req = InvokeRequest::new(
        "blocked_req_001",
        "tools.search",
        "fetch_page",
        json!({
            "url": "https://forbidden-malicious-tracker.com/leak"
        }),
    );

    let blocked_resp = router
        .dispatch("cli", blocked_req)
        .await
        .expect("Dispatch returned error");

    assert!(
        blocked_resp.error.is_some(),
        "Microkernel firewall must block unwhitelisted network domain"
    );
}
