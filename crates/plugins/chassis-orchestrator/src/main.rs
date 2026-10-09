mod agent;

use agent::{AgentOrchestrator, KernelClient};
use chassis_protocol::lifecycle::{
    CapabilityDeclaration, CapabilityRequirement, PluginAnnounceManifest, PluginAnnounceResult,
    METHOD_KERNEL_HANDSHAKE, METHOD_KERNEL_PING, METHOD_KERNEL_SHUTDOWN,
};
use chassis_protocol::{
    Id, Message, Request, Response, RpcError, METHOD_CAPABILITY_INVOKE,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::{mpsc, oneshot, Mutex};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    let (stdout_tx, mut stdout_rx) = mpsc::channel::<Message>(128);
    let pending_responses = Arc::new(Mutex::new(HashMap::<Id, oneshot::Sender<Response>>::new()));

    // Dedicated writer task flushing NDJSON messages to stdout
    tokio::spawn(async move {
        let mut writer = BufWriter::new(stdout);
        while let Some(msg) = stdout_rx.recv().await {
            if let Ok(ndjson) = msg.to_ndjson() {
                if writer.write_all(ndjson.as_bytes()).await.is_err() {
                    break;
                }
                if writer.flush().await.is_err() {
                    break;
                }
            }
        }
    });

    // Create a request channel for outgoing capability requests to the microkernel
    let (kernel_req_tx, mut kernel_req_rx) = mpsc::channel::<Request>(128);
    let stdout_tx_clone = stdout_tx.clone();
    tokio::spawn(async move {
        while let Some(req) = kernel_req_rx.recv().await {
            let _ = stdout_tx_clone.send(Message::Request(req)).await;
        }
    });

    let client = KernelClient::new(kernel_req_tx, Arc::clone(&pending_responses));
    let orchestrator = Arc::new(AgentOrchestrator::new(client));

    // Stdout / Stdin event multiplexer
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

        match message {
            Message::Request(req) => {
                let orch = Arc::clone(&orchestrator);
                let out_tx = stdout_tx.clone();
                let is_shutdown = req.method == METHOD_KERNEL_SHUTDOWN;

                tokio::spawn(async move {
                    let resp = handle_request(&req, &orch).await;
                    let _ = out_tx.send(Message::Response(resp)).await;
                });

                if is_shutdown {
                    break;
                }
            }
            Message::Response(resp) => {
                let mut pending = pending_responses.lock().await;
                if let Some(sender) = pending.remove(&resp.id) {
                    let _ = sender.send(resp);
                } else {
                    tracing::warn!("Received response for unknown request ID: {:?}", resp.id);
                }
            }
            Message::Notification(_notif) => {}
        }
    }

    Ok(())
}

async fn handle_request(req: &Request, orch: &AgentOrchestrator) -> Response {
    match req.method.as_str() {
        METHOD_KERNEL_HANDSHAKE => {
            let manifest = PluginAnnounceManifest {
                plugin_id: "chassis.agent.orchestrator".to_string(),
                version: "1.0.0".to_string(),
                display_name: "ReAct Agent Orchestrator".to_string(),
                description: "Autonomous ReAct agent loop for multi-step reasoning, tool dispatch, and conversation memory"
                    .to_string(),
                capabilities_offered: vec![CapabilityDeclaration {
                    id: "agent.orchestrate".to_string(),
                    version: "1.0.0".to_string(),
                    methods: vec!["run_goal".to_string(), "chat_turn".to_string()],
                    schema_ref: None,
                }],
                capabilities_required: vec![
                    CapabilityRequirement {
                        id: "model.generate".to_string(),
                        optional: false,
                        constraints: None,
                    },
                    CapabilityRequirement {
                        id: "tools.execute".to_string(),
                        optional: false,
                        constraints: None,
                    },
                    CapabilityRequirement {
                        id: "storage.conversation".to_string(),
                        optional: true,
                        constraints: None,
                    },
                ],
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
                "run_goal" => {
                    let goal = payload
                        .get("goal")
                        .and_then(|g| g.as_str())
                        .unwrap_or("Inspect the workspace and summarize status")
                        .to_string();
                    let max_steps = payload
                        .get("max_steps")
                        .and_then(|s| s.as_u64())
                        .unwrap_or(5) as usize;
                    let conv_id = payload
                        .get("conversation_id")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());

                    match orch.run_goal(&goal, max_steps, conv_id).await {
                        Ok(res) => Response::success(req.id.clone(), json!(res)),
                        Err(err) => Response::error(req.id.clone(), RpcError::internal_error(err)),
                    }
                }
                "chat_turn" => {
                    let message = payload
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Hello")
                        .to_string();
                    let conv_id = payload
                        .get("conversation_id")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());
                    let sys_prompt = payload
                        .get("system_prompt")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());

                    match orch.chat_turn(&message, conv_id, sys_prompt).await {
                        Ok(res) => Response::success(req.id.clone(), res),
                        Err(err) => Response::error(req.id.clone(), RpcError::internal_error(err)),
                    }
                }
                unknown => Response::error(
                    req.id.clone(),
                    RpcError::method_not_found(format!(
                        "Unknown capability method '{}' on capability 'agent.orchestrate'",
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
