use chassis_protocol::lifecycle::{
    CapabilityDeclaration, PluginAnnounceManifest, PluginAnnounceResult, METHOD_KERNEL_HANDSHAKE,
    METHOD_KERNEL_PING, METHOD_KERNEL_SHUTDOWN,
};
use chassis_protocol::{
    Message, Request, Response, METHOD_CAPABILITY_INVOKE,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let message = match Message::from_ndjson(trimmed) {
            Ok(msg) => msg,
            Err(_) => continue,
        };

        if let Message::Request(req) = message {
            let response = handle_request(&req).await;
            let ndjson = Message::Response(response).to_ndjson()?;
            stdout.write_all(ndjson.as_bytes()).await?;
            stdout.flush().await?;

            if req.method == METHOD_KERNEL_SHUTDOWN {
                break;
            }
        }
    }

    Ok(())
}

async fn handle_request(req: &Request) -> Response {
    match req.method.as_str() {
        METHOD_KERNEL_HANDSHAKE => {
            let manifest = PluginAnnounceManifest {
                plugin_id: "chassis.tools.filesystem".to_string(),
                version: "1.0.0".to_string(),
                display_name: "Filesystem Tools".to_string(),
                description: "Sandboxed workspace file reading, writing, and directory listing"
                    .to_string(),
                capabilities_offered: vec![CapabilityDeclaration {
                    id: "tools.execute".to_string(),
                    version: "1.0.0".to_string(),
                    methods: vec![
                        "file_read".to_string(),
                        "file_write".to_string(),
                        "list_dir".to_string(),
                    ],
                    schema_ref: None,
                }],
                capabilities_required: vec![],
                hooks_subscribed: vec![],
            };
            Response::success(req.id.clone(), json!(PluginAnnounceResult { manifest }))
        }

        METHOD_KERNEL_PING => Response::success(req.id.clone(), json!({ "status": "healthy" })),

        METHOD_KERNEL_SHUTDOWN => Response::success(req.id.clone(), json!({ "ready_to_exit": true })),

        METHOD_CAPABILITY_INVOKE => {
            let default_payload = json!({});
            let payload = req.params.as_ref().unwrap_or(&default_payload);
            let method = payload.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let call_payload = payload.get("payload").cloned().unwrap_or_else(|| json!({}));

            match method {
                "file_read" => {
                    let path_str = match call_payload.get("path").and_then(|p| p.as_str()) {
                        Some(p) => p,
                        None => {
                            return Response::error(
                                req.id.clone(),
                                chassis_protocol::RpcError::invalid_params("Missing 'path' parameter"),
                            );
                        }
                    };

                    match fs::read_to_string(path_str) {
                        Ok(content) => {
                            let len = content.len();
                            Response::success(
                                req.id.clone(),
                                json!({
                                    "path": path_str,
                                    "content": content,
                                    "bytes": len
                                }),
                            )
                        }
                        Err(err) => Response::error(
                            req.id.clone(),
                            chassis_protocol::RpcError::capability_not_found(format!(
                                "Failed to read file: {err}"
                            )),
                        ),
                    }
                }

                "file_write" => {
                    let path_str = match call_payload.get("path").and_then(|p| p.as_str()) {
                        Some(p) => p,
                        None => {
                            return Response::error(
                                req.id.clone(),
                                chassis_protocol::RpcError::invalid_params("Missing 'path' parameter"),
                            );
                        }
                    };
                    let content = match call_payload.get("content").and_then(|c| c.as_str()) {
                        Some(c) => c,
                        None => {
                            return Response::error(
                                req.id.clone(),
                                chassis_protocol::RpcError::invalid_params("Missing 'content' parameter"),
                            );
                        }
                    };

                    let path = Path::new(path_str);
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                    }

                    match fs::write(path, content) {
                        Ok(()) => Response::success(
                            req.id.clone(),
                            json!({
                                "path": path_str,
                                "bytes_written": content.len(),
                                "status": "written"
                            }),
                        ),
                        Err(err) => Response::error(
                            req.id.clone(),
                            chassis_protocol::RpcError::internal_error(format!(
                                "Failed to write file: {err}"
                            )),
                        ),
                    }
                }

                "list_dir" => {
                    let path_str = call_payload
                        .get("path")
                        .and_then(|p| p.as_str())
                        .unwrap_or(".");

                    match fs::read_dir(path_str) {
                        Ok(entries) => {
                            let mut list = Vec::new();
                            for entry in entries.flatten() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                let file_type = entry.file_type().ok();
                                let is_dir = file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false);
                                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);

                                list.push(json!({
                                    "name": name,
                                    "is_dir": is_dir,
                                    "size": size,
                                }));
                            }

                            Response::success(
                                req.id.clone(),
                                json!({
                                    "path": path_str,
                                    "entries": list
                                }),
                            )
                        }
                        Err(err) => Response::error(
                            req.id.clone(),
                            chassis_protocol::RpcError::internal_error(format!(
                                "Failed to read directory: {err}"
                            )),
                        ),
                    }
                }

                _ => Response::error(
                    req.id.clone(),
                    chassis_protocol::RpcError::method_not_found(format!(
                        "Unknown tools method '{method}'"
                    )),
                ),
            }
        }

        _ => Response::error(
            req.id.clone(),
            chassis_protocol::RpcError::method_not_found(format!(
                "Unknown kernel method '{}'",
                req.method
            )),
        ),
    }
}
