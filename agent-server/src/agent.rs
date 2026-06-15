use std::sync::Arc;

use agent_tools::Tool;
use futures::StreamExt;
use provider::{ChatRequest, FinishReason, Message, Provider, Role, StreamEvent, ToolDefinition};
use tokio::sync::oneshot;

use crate::session::{AgentEvent, PendingApproval, Session, SessionStatus};

pub async fn run(
    session: Arc<Session>,
    provider: Arc<dyn Provider + Send + Sync>,
    tools: Arc<Vec<Arc<dyn Tool>>>,
    system_prompt: Arc<String>,
) {
    *session.status.lock().await = SessionStatus::Running;

    loop {
        let req = build_request(&session, &tools, &system_prompt).await;

        let stream = match provider.stream_chat(&req) {
            Ok(s) => s,
            Err(e) => {
                emit_error(&session, e.to_string()).await;
                return;
            }
        };

        let mut stream = std::pin::pin!(stream);
        let mut final_response = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::Content(text)) => {
                    let _ = session.events.send(AgentEvent::Content { text });
                }
                Ok(StreamEvent::ToolCall(_)) => {
                    // tool calls arrive complete in Done; ignore partial streaming variants
                }
                Ok(StreamEvent::Done(resp)) => {
                    final_response = Some(resp);
                }
                Err(e) => {
                    emit_error(&session, e.to_string()).await;
                    return;
                }
            }
        }

        let response = match final_response {
            Some(r) => r,
            None => {
                emit_error(&session, "stream ended without Done event".into()).await;
                return;
            }
        };

        session.history.lock().await.push(Message {
            role: Role::Assistant,
            content: response.content.clone(),
            tool_calls: response.tool_calls.clone(),
            tool_call_id: None,
        });

        match response.finish_reason {
            FinishReason::Stop | FinishReason::Length => {
                *session.status.lock().await = SessionStatus::Idle;
                let _ = session.events.send(AgentEvent::TurnComplete);
                return;
            }
            FinishReason::Error => {
                emit_error(&session, "provider returned error finish reason".into()).await;
                return;
            }
            FinishReason::ToolCalls => {
                let tool_calls = response.tool_calls.unwrap_or_default();

                for tc in tool_calls {
                    let args = heal_args(&tc.arguments);

                    let _ = session.events.send(AgentEvent::ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: args.clone(),
                    });

                    // pause loop, wait for client approval
                    let (tx, rx) = oneshot::channel();
                    *session.pending_approval.lock().await =
                        Some(PendingApproval { tool_call_id: tc.id.clone(), tx });
                    *session.status.lock().await = SessionStatus::AwaitingApproval;

                    let approved = rx.await.unwrap_or(false);

                    if !approved {
                        emit_error(&session, format!("tool call {} denied", tc.id)).await;
                        return;
                    }

                    *session.status.lock().await = SessionStatus::Running;

                    let result = execute_tool(&tools, &tc.name, args).await;

                    let _ = session.events.send(AgentEvent::ToolResult {
                        id: tc.id.clone(),
                        content: result.clone(),
                    });

                    session.history.lock().await.push(Message {
                        role: Role::Tool,
                        content: result.to_string(),
                        tool_calls: None,
                        tool_call_id: Some(tc.id),
                    });
                }
                // loop: call LLM again with tool results appended
            }
        }
    }
}

async fn build_request(
    session: &Session,
    tools: &[Arc<dyn Tool>],
    system_prompt: &str,
) -> ChatRequest {
    let mut messages = vec![Message {
        role: Role::System,
        content: system_prompt.to_string(),
        tool_calls: None,
        tool_call_id: None,
    }];
    messages.extend(session.history.lock().await.iter().cloned());

    let tool_defs: Vec<ToolDefinition> = tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.name().to_string(),
            description: Some(t.description().to_string()),
            parameters: t.parameters(),
        })
        .collect();

    ChatRequest {
        model: session.model.clone(),
        messages,
        temperature: None,
        max_tokens: None,
        tools: if tool_defs.is_empty() { None } else { Some(tool_defs) },
    }
}

async fn execute_tool(
    tools: &[Arc<dyn Tool>],
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    match tools.iter().find(|t| t.name() == name) {
        Some(tool) => tool
            .call(args)
            .await
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() })),
        None => serde_json::json!({ "error": format!("unknown tool: {name}") }),
    }
}

/// Parse LLM-emitted arguments string, attempting recovery if invalid JSON.
fn heal_args(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| {
        // TODO: more sophisticated healing (strip trailing commas, fix quotes, etc.)
        serde_json::Value::Null
    })
}

async fn emit_error(session: &Session, message: String) {
    *session.status.lock().await = SessionStatus::Idle;
    let _ = session.events.send(AgentEvent::Error { message });
}
