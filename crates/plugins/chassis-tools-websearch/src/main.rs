mod search;

use chassis_protocol::lifecycle::{
    CapabilityDeclaration, PluginAnnounceManifest, PluginAnnounceResult, METHOD_KERNEL_HANDSHAKE,
    METHOD_KERNEL_PING, METHOD_KERNEL_SHUTDOWN,
};
use chassis_protocol::{
    Message, Request, Response, RpcError, METHOD_CAPABILITY_INVOKE,
};
use search::{execute_search, fetch_page_content};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = tokio::io::stdin();
    let mut stdout = BufWriter::new(tokio::io::stdout());
    let mut reader = BufReader::new(stdin).lines();

    let http_client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(1500))
        .timeout(Duration::from_secs(5))
        .build()?;

    while let Ok(Some(line)) = reader.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let message = match Message::from_ndjson(trimmed) {
            Ok(msg) => msg,
            Err(e) => {
                tracing::warn!("Failed to parse line as NDJSON: {} (err: {})", trimmed, e);
                continue;
            }
        };

        if let Message::Request(req) = message {
            let is_shutdown = req.method == METHOD_KERNEL_SHUTDOWN;
            let response = handle_request(&req, &http_client).await;
            let ndjson = Message::Response(response).to_ndjson()?;

            stdout.write_all(ndjson.as_bytes()).await?;
            stdout.flush().await?;

            if is_shutdown {
                break;
            }
        }
    }

    Ok(())
}

async fn handle_request(req: &Request, client: &reqwest::Client) -> Response {
    match req.method.as_str() {
        METHOD_KERNEL_HANDSHAKE => {
            let manifest = PluginAnnounceManifest {
                plugin_id: "chassis.tools.websearch".to_string(),
                version: "1.0.0".to_string(),
                display_name: "Free Web Search & RAG Tool".to_string(),
                description: "Free web search and information retrieval engine (DuckDuckGo & Wikipedia) for RAG agent workflows"
                    .to_string(),
                capabilities_offered: vec![CapabilityDeclaration {
                    id: "tools.search".to_string(),
                    version: "1.0.0".to_string(),
                    methods: vec!["web_search".to_string(), "fetch_page".to_string()],
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
            let default_params = json!({});
            let params = req.params.as_ref().unwrap_or(&default_params);
            let method = params.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let payload = params.get("payload").cloned().unwrap_or_else(|| json!({}));

            match method {
                "web_search" | "search" => {
                    let query = payload
                        .get("query")
                        .and_then(|q| q.as_str())
                        .unwrap_or("")
                        .to_string();

                    if query.trim().is_empty() {
                        return Response::error(
                            req.id.clone(),
                            RpcError::invalid_params("Parameter 'query' cannot be empty"),
                        );
                    }

                    let max_results = payload
                        .get("max_results")
                        .and_then(|m| m.as_u64())
                        .unwrap_or(5) as usize;

                    let results = execute_search(client, &query, max_results).await;
                    let count = results.len();

                    Response::success(
                        req.id.clone(),
                        json!({
                            "query": query,
                            "results": results,
                            "count": count
                        }),
                    )
                }

                "fetch_page" => {
                    let url = match payload.get("url").and_then(|u| u.as_str()) {
                        Some(u) => u,
                        None => {
                            return Response::error(
                                req.id.clone(),
                                RpcError::invalid_params("Missing 'url' parameter"),
                            );
                        }
                    };

                    match fetch_page_content(client, url).await {
                        Ok(content) => Response::success(req.id.clone(), json!(content)),
                        Err(e) => Response::error(req.id.clone(), RpcError::internal_error(e)),
                    }
                }

                unknown => Response::error(
                    req.id.clone(),
                    RpcError::method_not_found(format!(
                        "Unknown capability method '{}' on capability 'tools.search'",
                        unknown
                    )),
                ),
            }
        }

        unknown => Response::error(
            req.id.clone(),
            RpcError::method_not_found(format!("Unknown kernel method '{}'", unknown)),
        ),
    }
}
