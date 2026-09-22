use chassis_protocol::lifecycle::{METHOD_KERNEL_HANDSHAKE, METHOD_KERNEL_SHUTDOWN};
use chassis_protocol::{Message, Request, METHOD_CAPABILITY_INVOKE};
use serde_json::json;
use std::process::Stdio;
use tempfile::tempdir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[tokio::test]
async fn test_storage_plugin_live_stdio_jsonrpc_lifecycle() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test_rpc.db");

    let bin_path = env!("CARGO_BIN_EXE_chassis-storage-sqlite");

    let mut child = Command::new(bin_path)
        .env("CHASSIS_STORAGE_PATH", db_path.to_str().unwrap())
        .env("CHASSIS_STORAGE_DIMENSIONS", "3")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("Failed to spawn chassis-storage-sqlite plugin");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout).lines();

    // 1. Handshake
    let handshake_req = Request::new(1, METHOD_KERNEL_HANDSHAKE, Some(json!({})));
    let ndjson = Message::Request(handshake_req).to_ndjson().unwrap();
    stdin.write_all(ndjson.as_bytes()).await.unwrap();
    stdin.flush().await.unwrap();

    let response_line = reader.next_line().await.unwrap().unwrap();
    let msg = Message::from_ndjson(&response_line).unwrap();
    if let Message::Response(res) = msg {
        assert!(res.error.is_none());
        let result = res.result.unwrap();
        assert_eq!(result["manifest"]["plugin_id"], "chassis.storage.sqlite");
        assert_eq!(
            result["manifest"]["capabilities_offered"][0]["id"],
            "storage.conversation"
        );
    } else {
        panic!("Expected Response message for handshake");
    }

    // 2. Capability Invoke: conversation_create
    let create_conv_req = Request::new(
        2,
        METHOD_CAPABILITY_INVOKE,
        Some(json!({
            "capability": "storage.conversation",
            "method": "conversation_create",
            "payload": {
                "id": "rpc_conv_1",
                "title": "RPC Thread",
                "model_id": "llama-3",
                "chassis_session_id": "ses_wal_9999"
            }
        })),
    );
    stdin
        .write_all(Message::Request(create_conv_req).to_ndjson().unwrap().as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let line = reader.next_line().await.unwrap().unwrap();
    let msg = Message::from_ndjson(&line).unwrap();
    if let Message::Response(res) = msg {
        assert!(res.error.is_none());
        let result = res.result.unwrap();
        assert_eq!(result["id"], "rpc_conv_1");
        assert_eq!(result["title"], "RPC Thread");
        assert_eq!(result["chassis_session_id"], "ses_wal_9999");
    } else {
        panic!("Expected Response for conversation_create");
    }

    // 3. Capability Invoke: message_append with 3D vector embedding
    let append_msg_req = Request::new(
        3,
        METHOD_CAPABILITY_INVOKE,
        Some(json!({
            "capability": "storage.conversation",
            "method": "message_append",
            "payload": {
                "id": "rpc_m1",
                "conversation_id": "rpc_conv_1",
                "role": "user",
                "content": "Zero-trust microkernel security",
                "embedding": [1.0, 0.0, 0.0]
            }
        })),
    );
    stdin
        .write_all(Message::Request(append_msg_req).to_ndjson().unwrap().as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let line = reader.next_line().await.unwrap().unwrap();
    let msg = Message::from_ndjson(&line).unwrap();
    if let Message::Response(res) = msg {
        assert!(res.error.is_none());
        let result = res.result.unwrap();
        assert_eq!(result["id"], "rpc_m1");
        assert_eq!(result["role"], "user");
    } else {
        panic!("Expected Response for message_append");
    }

    // 4. Capability Invoke: message_search_similar (Vector Search)
    let search_req = Request::new(
        4,
        METHOD_CAPABILITY_INVOKE,
        Some(json!({
            "capability": "storage.conversation",
            "method": "message_search_similar",
            "payload": {
                "query_vector": [0.99, 0.01, 0.0],
                "top_k": 1
            }
        })),
    );
    stdin
        .write_all(Message::Request(search_req).to_ndjson().unwrap().as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let line = reader.next_line().await.unwrap().unwrap();
    let msg = Message::from_ndjson(&line).unwrap();
    if let Message::Response(res) = msg {
        assert!(res.error.is_none());
        let result = res.result.unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0]["message"]["id"], "rpc_m1");
        assert_eq!(matches[0]["message"]["content"], "Zero-trust microkernel security");
    } else {
        panic!("Expected Response for message_search_similar");
    }

    // 5. Graceful Shutdown
    let shutdown_req = Request::new(5, METHOD_KERNEL_SHUTDOWN, Some(json!({})));
    stdin
        .write_all(Message::Request(shutdown_req).to_ndjson().unwrap().as_bytes())
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let line = reader.next_line().await.unwrap().unwrap();
    let msg = Message::from_ndjson(&line).unwrap();
    if let Message::Response(res) = msg {
        assert_eq!(res.result.unwrap()["ready_to_exit"], true);
    }

    let status = child.wait().await.unwrap();
    assert!(status.success());
}
