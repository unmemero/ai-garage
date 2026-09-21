use chassis_core::{
    collect_descendants, init_subreaper, kill_process_tree, BlobStore, EffectiveLease,
    PluginManifest, PluginSupervisor, SecurityPolicy, SubAgentManager, SubAgentSpawnRequest,
    WalReader, WalWriter,
};
use chassis_protocol::InvokeRequest;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

/// STRESS-1: High-Concurrency Stdio Multiplexing (1,000 requests over asynchronous stdio)
#[tokio::test]
async fn test_stress_stdio_high_concurrency_multiplexing() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let script_path = ws.join("echo_plugin.sh");

    // Fast echo plugin over stdio NDJSON
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"echo.test","version":"1.0.0","display_name":"Echo","description":"Test","capabilities_offered":[{"id":"test.echo","version":"1.0.0","methods":["echo"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"capability/invoke"*)
      req_id=$(echo "$line" | grep -o '"id":[^,}]*' | head -n1 | cut -d: -f2 | tr -d ' "')
      echo "{\"jsonrpc\":\"2.0\",\"id\":$req_id,\"result\":{\"status\":\"echoed\"}}"
      ;;
    *"kernel/shutdown"*)
      echo '{"jsonrpc":"2.0","id":9999,"result":{"ready_to_exit":true}}'
      exit 0
      ;;
  esac
done
"#;
    fs::write(&script_path, script).unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let manifest = PluginManifest::from_toml(
        r#"
[plugin]
id = "echo.test"
name = "Echo"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./echo_plugin.sh"

[[capabilities_offered]]
id = "test.echo"
version = "1.0.0"
methods = ["echo"]
"#,
    )
    .unwrap();

    let mut policy = SecurityPolicy::default();
    policy.workspace.root = ws.to_path_buf();

    let supervisor = Arc::new(
        PluginSupervisor::launch_and_handshake(
            manifest,
            &policy,
            ws,
            ws,
            "ses_stress_multiplex",
            Duration::from_secs(5),
        )
        .await
        .expect("Handshake failed"),
    );

    let num_requests = 1000;
    let completed = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::with_capacity(num_requests);

    for i in 0..num_requests {
        let sup = Arc::clone(&supervisor);
        let done = Arc::clone(&completed);
        tasks.push(tokio::spawn(async move {
            let req = InvokeRequest::new(
                format!("req_{i}"),
                "test.echo",
                "echo",
                json!({ "index": i }),
            );
            let resp = tokio::time::timeout(Duration::from_secs(5), sup.invoke(&req))
                .await
                .expect("Multiplexed request timed out")
                .expect("Multiplexed request failed");
            assert!(resp.error.is_none());
            done.fetch_add(1, Ordering::Relaxed);
        }));
    }

    for t in tasks {
        t.await.unwrap();
    }

    assert_eq!(completed.load(Ordering::Relaxed), num_requests);
}

/// STRESS-2: Concurrent Multi-Megabyte Blob Spillover (50 concurrent 512KB-2MB payloads)
#[tokio::test]
async fn test_stress_concurrent_blob_spillover() {
    let dir = tempdir().unwrap();
    let blobs_dir = dir.path().join("blobs");
    let store = Arc::new(BlobStore::new(&blobs_dir).unwrap());

    let num_blobs = 50;
    let mut tasks = Vec::with_capacity(num_blobs);

    for i in 0..num_blobs {
        let s = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let size = 512 * 1024 + (i * 32 * 1024); // 512KB to 2MB
            let data = vec![(i % 256) as u8; size];

            let descriptor = s
                .store(&data, "application/octet-stream")
                .expect("Failed to store large blob");
            assert_eq!(descriptor.byte_length, size as u64);
            assert!(descriptor.blob.starts_with("sha256:"));

            let read_back = s.read(&descriptor).expect("Failed to read back blob");
            assert_eq!(read_back.len(), size);
            assert_eq!(read_back[0], (i % 256) as u8);
        }));
    }

    for t in tasks {
        t.await.unwrap();
    }
}

/// STRESS-3: Sub-Agent Spawning Storm (50 concurrent nested sub-agents & sub-WALs)
#[tokio::test]
async fn test_stress_subagent_spawning_storm() {
    let dir = tempdir().unwrap();
    let sessions_dir = dir.path();
    let num_subagents = 50;

    let mut tasks = Vec::with_capacity(num_subagents);

    for i in 0..num_subagents {
        let s_dir = sessions_dir.to_path_buf();
        tasks.push(tokio::spawn(async move {
            let parent_id = format!("ses_parent_storm_{i}");
            let mut parent_wal = WalWriter::init(&parent_id, &s_dir).unwrap();
            parent_wal
                .append("SESSION_START", json!({ "id": i }))
                .unwrap();

            let parent_lease = EffectiveLease {
                allow_filesystem_read: true,
                ..Default::default()
            };

            let mgr = SubAgentManager::new(&parent_id, &s_dir);
            let req = SubAgentSpawnRequest {
                subagent_id: format!("child_{i}"),
                goal: format!("Task {i}"),
                granted_capabilities: vec!["tools.execute:file_read".to_string()],
                timeout_secs: 5,
            };

            let res = mgr
                .spawn_and_execute(req, &parent_lease, &mut parent_wal, |sub_id, _wal_path| async move {
                    Ok(Some(format!("Done {sub_id}")))
                })
                .await
                .expect("Sub-agent execution failed");

            assert_eq!(res.status, "success");
            assert_eq!(res.events_count, 3);
        }));
    }

    for t in tasks {
        t.await.unwrap();
    }
}

/// STRESS-4: Abrupt Process Kill (`SIGKILL`) Under Load with Clean Recovery
#[tokio::test]
async fn test_stress_abrupt_process_kill_recovery() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let script_path = ws.join("hanging_plugin.sh");

    // Plugin that sleeps on request
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"hanging.test","version":"1.0.0","display_name":"Hang","description":"Test","capabilities_offered":[{"id":"test.hang","version":"1.0.0","methods":["hang"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"capability/invoke"*)
      sleep 10
      ;;
    *"kernel/shutdown"*)
      exit 0
      ;;
  esac
done
"#;
    fs::write(&script_path, script).unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let manifest = PluginManifest::from_toml(
        r#"
[plugin]
id = "hanging.test"
name = "Hang"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./hanging_plugin.sh"

[[capabilities_offered]]
id = "test.hang"
version = "1.0.0"
methods = ["hang"]
"#,
    )
    .unwrap();

    let mut policy = SecurityPolicy::default();
    policy.workspace.root = ws.to_path_buf();

    let supervisor = PluginSupervisor::launch_and_handshake(
        manifest,
        &policy,
        ws,
        ws,
        "ses_stress_kill",
        Duration::from_secs(5),
    )
    .await
    .expect("Handshake failed");

    // Spawn in-flight request
    let req = InvokeRequest::new("req_hang", "test.hang", "hang", json!({}));
    let sup_arc = Arc::new(supervisor);
    let sup_clone = Arc::clone(&sup_arc);

    let req_task = tokio::spawn(async move {
        // High timeout (5s) so if pipe EOF drain works, it finishes in <200ms rather than timing out
        tokio::time::timeout(Duration::from_secs(5), sup_clone.invoke(&req)).await
    });

    // Let the request enter flight
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send SIGKILL directly to the child process group
    let kill_start = std::time::Instant::now();
    if let Some(pid) = sup_arc.process.pid() {
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }

    // Await request completion: instant pipe EOF drain MUST complete in <300ms, not 5 seconds!
    let result = req_task.await.unwrap();
    let elapsed = kill_start.elapsed();
    assert!(
        elapsed < Duration::from_millis(500),
        "Immediate EOF drain failed: took {:?} instead of <500ms",
        elapsed
    );

    let resp = result
        .expect("Request should not timeout because EOF drain immediately notifies pending calls")
        .expect("Invocation must return JSON-RPC response with error code");
    assert!(resp.error.is_some(), "Killed process should return error");
    assert_eq!(
        resp.error.unwrap().code,
        -32003,
        "Expected PluginUnavailable error code -32003"
    );
}

/// STRESS-5: Sequential WAL High-Throughput Append & Hash Chaining (1,000 events)
#[test]
fn test_stress_wal_high_throughput_integrity() {
    let dir = tempdir().unwrap();
    let sessions_dir = dir.path();

    let mut writer = WalWriter::init("ses_stress_wal", sessions_dir).unwrap();
    let num_events = 1000;

    for i in 0..num_events {
        writer
            .append(
                "STRESS_EVENT",
                json!({ "seq_num": i, "payload": "sovereign_event_data" }),
            )
            .unwrap();
    }
    assert_eq!(writer.current_seq(), num_events as u64);

    let wal_path = writer.file_path().to_path_buf();
    drop(writer);

    let reader = WalReader::open(&wal_path).unwrap();
    let validated = reader.validate_integrity().unwrap();
    assert_eq!(validated.len(), num_events);
    for (i, ev) in validated.iter().enumerate() {
        assert_eq!(ev.seq, i as u64);
        assert!(ev.hash.starts_with("sha256:"));
    }
}

/// STRESS-6: Descendant Process Tree & Grandchild Reaping (`kill_process_tree` + Subreaper)
#[tokio::test]
async fn test_stress_descendant_process_tree_reaping() {
    init_subreaper();

    let dir = tempdir().unwrap();
    let ws = dir.path();
    let script_path = ws.join("forking_plugin.sh");

    // Plugin that spawns a background grandchild process (sleep 60)
    let script = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *"kernel/handshake"*)
      sleep 60 &
      echo '{"jsonrpc":"2.0","id":1,"result":{"manifest":{"plugin_id":"forking.test","version":"1.0.0","display_name":"Forking","description":"Test","capabilities_offered":[{"id":"test.fork","version":"1.0.0","methods":["fork"]}],"capabilities_required":[],"hooks_subscribed":[]}}}'
      ;;
    *"kernel/shutdown"*)
      exit 0
      ;;
  esac
done
"#;
    fs::write(&script_path, script).unwrap();
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let manifest = PluginManifest::from_toml(
        r#"
[plugin]
id = "forking.test"
name = "Forking"
version = "1.0.0"

[entrypoint]
runtime = "script"
executable = "./forking_plugin.sh"

[[capabilities_offered]]
id = "test.fork"
version = "1.0.0"
methods = ["fork"]
"#,
    )
    .unwrap();

    let mut policy = SecurityPolicy::default();
    policy.workspace.root = ws.to_path_buf();

    let supervisor = PluginSupervisor::launch_and_handshake(
        manifest,
        &policy,
        ws,
        ws,
        "ses_stress_fork",
        Duration::from_secs(5),
    )
    .await
    .expect("Handshake failed");

    let child_pid = supervisor.process.pid().expect("Child PID must exist");

    // Give kernel a moment to spawn grandchild
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Discover descendants
    let descendants = collect_descendants(child_pid);
    assert!(!descendants.is_empty(), "Child must have spawned grandchild process");
    let grandchild_pid = descendants[0];

    // Grandchild must be alive in /proc
    assert!(
        std::path::Path::new(&format!("/proc/{}", grandchild_pid)).exists(),
        "Grandchild process {} must be running in /proc",
        grandchild_pid
    );

    // Drop supervisor to trigger ProcessHandle::drop (force_kill) and tokio child reap
    drop(supervisor);

    // Also explicitly ensure kill_process_tree reaps anything adopted
    kill_process_tree(child_pid);

    // Allow kernel & tokio to reap
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Both child and grandchild must be gone from /proc
    assert!(
        !std::path::Path::new(&format!("/proc/{}", child_pid)).exists(),
        "Child process {} must no longer exist in /proc",
        child_pid
    );
    assert!(
        !std::path::Path::new(&format!("/proc/{}", grandchild_pid)).exists(),
        "Grandchild process {} must be reaped and no longer exist in /proc",
        grandchild_pid
    );
}
