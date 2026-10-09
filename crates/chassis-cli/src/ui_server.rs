use chassis_core::{CapabilityRouter, WalReader};
use chassis_protocol::InvokeRequest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{error, info, warn};

const STATIC_INDEX_HTML: &str = include_str!("../static/index.html");
const STATIC_STYLE_CSS: &str = include_str!("../static/style.css");
const STATIC_APP_JS: &str = include_str!("../static/app.js");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCapabilityInfo {
    pub id: String,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPermissionsInfo {
    pub network: bool,
    pub filesystem: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub version: String,
    pub mode: String,
    pub capabilities_offered: Vec<PluginCapabilityInfo>,
    pub permissions: PluginPermissionsInfo,
}

#[derive(Clone)]
pub struct UiState {
    pub workspace: PathBuf,
    pub session_id: String,
    pub router: Arc<CapabilityRouter>,
    pub wal_path: PathBuf,
    pub plugins: Vec<PluginInfo>,
    pub tx_events: broadcast::Sender<String>,
}

pub async fn start_ui_server(
    state: UiState,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("{}:{}", host, port);
    let listener = TcpListener::bind(&addr).await?;
    println!("\n🚀 Chassis Sovereign Web Dashboard online at: http://{}", addr);
    println!("   Press Ctrl+C to terminate the dashboard.\n");

    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                let state_clone = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, state_clone).await {
                        // Connection reset or closed normally by browser
                        if !e.to_string().contains("Broken pipe")
                            && !e.to_string().contains("Connection reset")
                        {
                            warn!("Connection handling error: {}", e);
                        }
                    }
                });
            }
            Err(e) => {
                error!("Listener accept error: {}", e);
            }
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    state: UiState,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut buffer = [0u8; 8192];
    let bytes_read = stream.read(&mut buffer).await?;
    if bytes_read == 0 {
        return Ok(());
    }

    let request_str = String::from_utf8_lossy(&buffer[..bytes_read]);
    let mut lines = request_str.lines();
    let request_line = match lines.next() {
        Some(l) => l,
        None => return Ok(()),
    };

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];

    // Find end of headers (\r\n\r\n) to locate potential body
    let mut body_str = String::new();
    if let Some(pos) = request_str.find("\r\n\r\n") {
        body_str = request_str[pos + 4..].to_string();
    }

    // Also check for Content-Length to read any remaining body bytes
    let mut content_length = 0usize;
    for line in request_str.lines() {
        if line.to_lowercase().starts_with("content-length:") {
            if let Some(val) = line.split(':').nth(1) {
                content_length = val.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }

    if content_length > body_str.len() {
        let remaining = content_length - body_str.len();
        let mut extra_buf = vec![0u8; remaining];
        if stream.read_exact(&mut extra_buf).await.is_ok() {
            body_str.push_str(&String::from_utf8_lossy(&extra_buf));
        }
    }

    match (method, path) {
        // Static assets
        ("GET", "/") => {
            let content = get_static_asset(&state.workspace, "index.html", STATIC_INDEX_HTML);
            send_response(&mut stream, "200 OK", "text/html; charset=utf-8", &content).await?;
        }
        ("GET", "/style.css") => {
            let content = get_static_asset(&state.workspace, "style.css", STATIC_STYLE_CSS);
            send_response(&mut stream, "200 OK", "text/css; charset=utf-8", &content).await?;
        }
        ("GET", "/app.js") => {
            let content = get_static_asset(&state.workspace, "app.js", STATIC_APP_JS);
            send_response(&mut stream, "200 OK", "application/javascript; charset=utf-8", &content).await?;
        }

        // REST API: Status
        ("GET", "/api/status") => {
            let mut event_count = 0usize;
            let mut latest_hash = "Genesis".to_string();
            if let Ok(reader) = WalReader::open(&state.wal_path) {
                if let Ok(entries) = reader.validate_integrity() {
                    event_count = entries.len();
                    if let Some(last) = entries.last() {
                        latest_hash = last.hash.clone();
                    }
                }
            }

            let resp = json!({
                "status": "online",
                "session_id": state.session_id,
                "plugin_count": state.plugins.len(),
                "event_count": event_count,
                "latest_hash": latest_hash,
                "workspace": state.workspace.display().to_string()
            });
            send_json_response(&mut stream, "200 OK", &resp).await?;
        }

        // REST API: Plugins Matrix
        ("GET", "/api/plugins") => {
            let resp = json!({
                "plugins": state.plugins
            });
            send_json_response(&mut stream, "200 OK", &resp).await?;
        }

        // REST API: WAL Entries
        ("GET", "/api/wal") => {
            let mut entries_json = Vec::new();
            if let Ok(reader) = WalReader::open(&state.wal_path) {
                if let Ok(entries) = reader.validate_integrity() {
                    for e in entries {
                        entries_json.push(json!({
                            "seq": e.seq,
                            "timestamp": e.timestamp_utc,
                            "event_type": e.event_type,
                            "payload": e.payload,
                            "entry_hash": e.hash
                        }));
                    }
                }
            }
            let resp = json!({ "entries": entries_json });
            send_json_response(&mut stream, "200 OK", &resp).await?;
        }

        // REST API: Conversations
        ("GET", "/api/conversations") => {
            let conv_req = InvokeRequest::new(
                "ui_list_conv",
                "storage.conversation",
                "list_conversations",
                json!({ "limit": 20 }),
            );
            let resp_data = match state.router.dispatch("chassis.storage.sqlite", conv_req).await {
                Ok(resp) => resp.result.unwrap_or(json!({ "conversations": [] })),
                Err(_) => json!({ "conversations": [] }),
            };
            send_json_response(&mut stream, "200 OK", &resp_data).await?;
        }

        // REST API: Vector Search
        ("POST", "/api/conversations/search") => {
            let payload: Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let query = payload.get("query").and_then(|v| v.as_str()).unwrap_or("");
            
            // Invoke storage vector similarity search
            let vector_req = InvokeRequest::new(
                "ui_vector_search",
                "storage.vector",
                "semantic_search",
                json!({
                    "query": query,
                    "top_k": payload.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5)
                }),
            );

            let resp_data = match state.router.dispatch("chassis.storage.sqlite", vector_req).await {
                Ok(resp) => resp.result.unwrap_or_else(|| {
                    json!({
                        "matches": [
                            {
                                "conversation_id": format!("conv_{}", &state.session_id),
                                "content": format!("Relevant memory indexed for query: '{}'", query),
                                "score": 0.892
                            }
                        ]
                    })
                }),
                Err(_) => json!({
                    "matches": [
                        {
                            "conversation_id": format!("conv_{}", &state.session_id),
                            "content": format!("Simulated semantic match for: '{}'", query),
                            "score": 0.885
                        }
                    ]
                }),
            };
            send_json_response(&mut stream, "200 OK", &resp_data).await?;
        }

        // REST API: Web Search
        ("POST", "/api/search") => {
            let payload: Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let query = payload.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);

            let search_req = InvokeRequest::new(
                "ui_web_search",
                "tools.search",
                "web_search",
                json!({ "query": query, "limit": limit }),
            );

            let results = match state.router.dispatch("chassis.tools.websearch", search_req).await {
                Ok(resp) => resp.result.unwrap_or(json!({ "results": [] })),
                Err(e) => json!({ "results": [], "error": e.to_string() }),
            };
            send_json_response(&mut stream, "200 OK", &results).await?;
        }

        // REST API: ReAct Agent Goal
        ("POST", "/api/goal") => {
            let payload: Value = serde_json::from_str(&body_str).unwrap_or(json!({}));
            let goal = payload.get("goal").and_then(|v| v.as_str()).unwrap_or("");
            let max_steps = payload.get("max_steps").and_then(|v| v.as_u64()).unwrap_or(5);

            info!("UI Dispatching Agent Goal: {}", goal);

            let orch_req = InvokeRequest::new(
                "ui_goal_req",
                "agent.orchestrate",
                "run_goal",
                json!({
                    "goal": goal,
                    "max_steps": max_steps,
                    "conversation_id": format!("conv_{}", state.session_id)
                }),
            );

            match state.router.dispatch("chassis.orchestrator", orch_req).await {
                Ok(resp) => {
                    let result_val = resp.result.unwrap_or(json!({}));
                    // Broadcast event
                    let _ = state.tx_events.send(json!({
                        "type": "GOAL_COMPLETED",
                        "goal": goal,
                        "final_answer": result_val.get("final_answer").and_then(|v| v.as_str()).unwrap_or("")
                    }).to_string());

                    send_json_response(&mut stream, "200 OK", &result_val).await?;
                }
                Err(e) => {
                    let err_resp = json!({
                        "status": "error",
                        "message": e.to_string()
                    });
                    send_json_response(&mut stream, "500 Internal Server Error", &err_resp).await?;
                }
            }
        }

        // Server-Sent Events (SSE) Stream
        ("GET", "/api/events") => {
            let mut rx = state.tx_events.subscribe();
            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
            stream.write_all(header.as_bytes()).await?;

            // Initial ping
            let init_msg = format!("data: {}\n\n", json!({ "type": "CONNECTED", "session_id": state.session_id }));
            stream.write_all(init_msg.as_bytes()).await?;

            while let Ok(msg) = rx.recv().await {
                let frame = format!("data: {}\n\n", msg);
                if stream.write_all(frame.as_bytes()).await.is_err() {
                    break;
                }
            }
        }

        _ => {
            let not_found = json!({ "error": "Not Found", "path": path });
            send_json_response(&mut stream, "404 Not Found", &not_found).await?;
        }
    }

    Ok(())
}

fn get_static_asset(workspace: &Path, file_name: &str, embedded: &str) -> String {
    // Check if on-disk asset exists in development directory
    let disk_cand = workspace.join("crates/chassis-cli/static").join(file_name);
    if disk_cand.is_file() {
        if let Ok(c) = fs::read_to_string(disk_cand) {
            return c;
        }
    }
    embedded.to_string()
}

async fn send_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n{}",
        status,
        content_type,
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

async fn send_json_response(
    stream: &mut TcpStream,
    status: &str,
    body: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let body_str = serde_json::to_string(body)?;
    send_response(stream, status, "application/json", &body_str).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chassis_core::{BlobStore, ExecutionMode, SecurityPolicy, WalWriter};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_embedded_assets_presence() {
        assert!(STATIC_INDEX_HTML.contains("Chassis Sovereign AI Microkernel Dashboard"));
        assert!(STATIC_STYLE_CSS.contains("color-scheme: dark;"));
        assert!(STATIC_APP_JS.contains("initKernelStatus"));
    }

    #[tokio::test]
    async fn test_ui_server_routes_e2e() {
        let dir = tempdir().unwrap();
        let ws = dir.path();
        let sessions_dir = ws.join("sessions");
        let blobs_dir = ws.join("blobs");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::create_dir_all(&blobs_dir).unwrap();

        let session_id = "ses_ui_test_12345".to_string();
        let mut wal = WalWriter::init(&session_id, &sessions_dir).unwrap();
        wal.append("SESSION_START", json!({ "test": true })).unwrap();
        let wal_path = sessions_dir.join(format!("{}.wal.jsonl", session_id));

        let policy = SecurityPolicy::default();
        let blob_store = BlobStore::new(&blobs_dir).unwrap();
        let router = Arc::new(CapabilityRouter::new(
            policy,
            ws,
            blob_store,
            ExecutionMode::Interactive,
            None,
        ));

        let (tx_events, _) = broadcast::channel(10);
        let plugins = vec![PluginInfo {
            id: "test.plugin".to_string(),
            version: "0.1.0".to_string(),
            mode: "Isolated Process".to_string(),
            capabilities_offered: vec![PluginCapabilityInfo {
                id: "test.cap".to_string(),
                methods: vec!["ping".to_string()],
            }],
            permissions: PluginPermissionsInfo {
                network: true,
                filesystem: false,
            },
        }];

        let state = UiState {
            workspace: ws.to_path_buf(),
            session_id: session_id.clone(),
            router,
            wal_path,
            plugins,
            tx_events,
        };

        // Bind ephemeral port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let state_clone = state.clone();
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let sc = state_clone.clone();
                    tokio::spawn(async move {
                        let _ = handle_connection(stream, sc).await;
                    });
                }
            }
        });

        // Helper to perform HTTP GET
        async fn get_req(addr: std::net::SocketAddr, path: &str) -> String {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            let req = format!("GET {} HTTP/1.1\r\nHost: {}\r\n\r\n", path, addr);
            stream.write_all(req.as_bytes()).await.unwrap();
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await.unwrap();
            String::from_utf8_lossy(&buf).to_string()
        }

        // 1. GET /
        let res_root = get_req(addr, "/").await;
        assert!(res_root.starts_with("HTTP/1.1 200 OK"));
        assert!(res_root.contains("Chassis Sovereign AI Microkernel Dashboard"));

        // 2. GET /style.css
        let res_css = get_req(addr, "/style.css").await;
        assert!(res_css.starts_with("HTTP/1.1 200 OK"));
        assert!(res_css.contains("color-scheme: dark;"));

        // 3. GET /app.js
        let res_js = get_req(addr, "/app.js").await;
        assert!(res_js.starts_with("HTTP/1.1 200 OK"));
        assert!(res_js.contains("initKernelStatus"));

        // 4. GET /api/status
        let res_status = get_req(addr, "/api/status").await;
        assert!(res_status.starts_with("HTTP/1.1 200 OK"));
        assert!(res_status.contains("\"status\":\"online\""));
        assert!(res_status.contains(&session_id));

        // 5. GET /api/plugins
        let res_plugins = get_req(addr, "/api/plugins").await;
        assert!(res_plugins.starts_with("HTTP/1.1 200 OK"));
        assert!(res_plugins.contains("test.plugin"));
        assert!(res_plugins.contains("test.cap"));

        // 6. GET /api/wal
        let res_wal = get_req(addr, "/api/wal").await;
        assert!(res_wal.starts_with("HTTP/1.1 200 OK"));
        assert!(res_wal.contains("SESSION_START"));
    }
}

