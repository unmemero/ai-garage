use chassis_protocol::lifecycle::{
    CapabilityDeclaration, PluginAnnounceManifest, PluginAnnounceResult, METHOD_KERNEL_HANDSHAKE,
    METHOD_KERNEL_PING, METHOD_KERNEL_SHUTDOWN,
};
use chassis_protocol::{
    Message, Request, Response, RpcError, METHOD_CAPABILITY_INVOKE,
};
use chassis_storage_sqlite::{ConversationStore, Role, StorageConfig};
use serde_json::json;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::OnceCell;

static STORE: OnceCell<Arc<ConversationStore>> = OnceCell::const_new();

async fn get_or_init_store() -> Result<Arc<ConversationStore>, Box<dyn std::error::Error + Send + Sync>> {
    STORE
        .get_or_try_init(|| async {
            let db_path = if let Ok(custom_path) = env::var("CHASSIS_STORAGE_PATH") {
                if custom_path == ":memory:" {
                    None
                } else {
                    Some(PathBuf::from(custom_path))
                }
            } else {
                Some(PathBuf::from(".chassis/storage/conversations.db"))
            };

            let dims = env::var("CHASSIS_STORAGE_DIMENSIONS")
                .ok()
                .and_then(|d| d.parse::<usize>().ok())
                .unwrap_or(384);

            let store = ConversationStore::open(StorageConfig {
                db_path,
                vector_dimensions: dims,
            })
            .await?;

            Ok(Arc::new(store))
        })
        .await
        .map(Arc::clone)
}

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
                plugin_id: "chassis.storage.sqlite".to_string(),
                version: "1.0.0".to_string(),
                display_name: "SQLite Conversation Storage".to_string(),
                description: "Sovereign UI conversation storage and semantic memory engine with libSQL vector search"
                    .to_string(),
                capabilities_offered: vec![CapabilityDeclaration {
                    id: "storage.conversation".to_string(),
                    version: "1.0.0".to_string(),
                    methods: vec![
                        "conversation_create".to_string(),
                        "conversation_get".to_string(),
                        "conversation_list".to_string(),
                        "conversation_update_title".to_string(),
                        "conversation_delete".to_string(),
                        "message_append".to_string(),
                        "message_get_history".to_string(),
                        "message_search_similar".to_string(),
                    ],
                    schema_ref: None,
                }],
                capabilities_required: vec![],
                hooks_subscribed: vec![],
            };
            Response::success(req.id.clone(), json!(PluginAnnounceResult { manifest }))
        }

        METHOD_KERNEL_PING => Response::success(req.id.clone(), json!({ "status": "healthy" })),

        METHOD_KERNEL_SHUTDOWN => {
            Response::success(req.id.clone(), json!({ "ready_to_exit": true }))
        }

        METHOD_CAPABILITY_INVOKE => {
            let store = match get_or_init_store().await {
                Ok(s) => s,
                Err(e) => {
                    return Response::error(
                        req.id.clone(),
                        RpcError::internal_error(format!("Failed to initialize storage: {e}")),
                    );
                }
            };

            let default_payload = json!({});
            let params_val = req.params.as_ref().unwrap_or(&default_payload);
            let method = params_val
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            let call_payload = params_val
                .get("payload")
                .cloned()
                .unwrap_or_else(|| json!({}));

            match method {
                "conversation_create" => {
                    let id = call_payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let title = call_payload
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("New Conversation");
                    let model_id = call_payload
                        .get("model_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("default");
                    let system_prompt = call_payload
                        .get("system_prompt")
                        .and_then(|v| v.as_str());
                    let chassis_session_id = call_payload
                        .get("chassis_session_id")
                        .and_then(|v| v.as_str());
                    let metadata = call_payload.get("metadata").cloned();

                    match store
                        .create_conversation(
                            id,
                            title,
                            model_id,
                            system_prompt,
                            chassis_session_id,
                            metadata,
                        )
                        .await
                    {
                        Ok(conv) => Response::success(req.id.clone(), json!(conv)),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "conversation_get" => {
                    let id = call_payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    match store.get_conversation(id).await {
                        Ok(Some(conv)) => Response::success(req.id.clone(), json!(conv)),
                        Ok(None) => Response::error(
                            req.id.clone(),
                            RpcError::invalid_params("Conversation not found"),
                        ),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "conversation_list" => {
                    let limit = call_payload
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(20) as u32;
                    let offset = call_payload
                        .get("offset")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;

                    match store.list_conversations(limit, offset).await {
                        Ok(list) => Response::success(req.id.clone(), json!({ "conversations": list })),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "conversation_update_title" => {
                    let id = call_payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let title = call_payload
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match store.update_conversation_title(id, title).await {
                        Ok(true) => Response::success(req.id.clone(), json!({ "updated": true })),
                        Ok(false) => Response::error(
                            req.id.clone(),
                            RpcError::invalid_params("Conversation not found"),
                        ),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "conversation_delete" => {
                    let id = call_payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    match store.delete_conversation(id).await {
                        Ok(deleted) => Response::success(req.id.clone(), json!({ "deleted": deleted })),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "message_append" => {
                    let id = call_payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let conversation_id = call_payload
                        .get("conversation_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let role_str = call_payload
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("user");
                    let role: Role = match role_str.parse() {
                        Ok(r) => r,
                        Err(e) => {
                            return Response::error(req.id.clone(), RpcError::invalid_params(e));
                        }
                    };
                    let content = call_payload
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let token_count = call_payload
                        .get("token_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let wal_seq = call_payload.get("wal_seq").and_then(|v| v.as_i64());
                    let metadata = call_payload.get("metadata").cloned();

                    let embedding_vec: Option<Vec<f32>> = call_payload
                        .get("embedding")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| item.as_f64().map(|f| f as f32))
                                .collect()
                        });

                    match store
                        .append_message(
                            id,
                            conversation_id,
                            role,
                            content,
                            token_count,
                            wal_seq,
                            metadata,
                            embedding_vec.as_deref(),
                        )
                        .await
                    {
                        Ok(msg) => Response::success(req.id.clone(), json!(msg)),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "message_get_history" => {
                    let conversation_id = call_payload
                        .get("conversation_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let limit = call_payload
                        .get("limit")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(50) as u32;
                    let offset = call_payload
                        .get("offset")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;

                    match store.get_messages(conversation_id, limit, offset).await {
                        Ok(messages) => Response::success(req.id.clone(), json!({ "messages": messages })),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                "message_search_similar" => {
                    let query_vec: Vec<f32> = match call_payload.get("query_vector").and_then(|v| v.as_array()) {
                        Some(arr) => arr
                            .iter()
                            .filter_map(|item| item.as_f64().map(|f| f as f32))
                            .collect(),
                        None => {
                            return Response::error(
                                req.id.clone(),
                                RpcError::invalid_params("Missing 'query_vector' parameter"),
                            );
                        }
                    };

                    let top_k = call_payload
                        .get("top_k")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(5) as usize;
                    let filter_conv = call_payload
                        .get("conversation_id")
                        .and_then(|v| v.as_str());

                    match store
                        .search_similar_messages(&query_vec, top_k, filter_conv)
                        .await
                    {
                        Ok(matches) => Response::success(req.id.clone(), json!({ "matches": matches })),
                        Err(e) => Response::error(
                            req.id.clone(),
                            RpcError::internal_error(e.to_string()),
                        ),
                    }
                }

                unknown => Response::error(
                    req.id.clone(),
                    RpcError::method_not_found(format!("Unknown storage method: {unknown}")),
                ),
            }
        }

        unknown => Response::error(
            req.id.clone(),
            RpcError::method_not_found(format!("Unknown kernel method: {unknown}")),
        ),
    }
}
