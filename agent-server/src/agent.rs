use std::sync::Arc;

use agent_tools::Tool;
use futures::StreamExt;
use provider::{ChatRequest, FinishReason, Message, Provider, Role, StreamEvent, ToolDefinition};
use tokio::sync::oneshot;

use std::time::Instant;

use crate::prompt::SystemPrompt;
use crate::session::{AgentEvent, Metrics, PendingApproval, Session, SessionStatus};

pub async fn run(
    session: Arc<Session>,
    provider: Arc<dyn Provider + Send + Sync>,
    tools: Arc<Vec<Arc<dyn Tool>>>,
    system_prompt: Arc<SystemPrompt>,
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
        let started = Instant::now();
        let mut first_token: Option<Instant> = None;

        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::Content(text)) => {
                    first_token.get_or_insert_with(Instant::now);
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

        let ended = Instant::now();
        emit_metrics(&session, &response.usage, started, first_token, ended);

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
    system_prompt: &SystemPrompt,
) -> ChatRequest {
    let system_msg: Message = system_prompt.clone().add_tools(tools).into();
    let mut messages = vec![system_msg];
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

fn emit_metrics(
    session: &Session,
    usage: &Option<provider::Usage>,
    started: Instant,
    first_token: Option<Instant>,
    ended: Instant,
) {
    let completion_tokens = usage.as_ref().map(|u| u.completion_tokens);
    let ttft_ms = first_token.map(|t| t.duration_since(started).as_millis() as u64);

    // Average throughput over the whole call (request start → end). We don't use
    // the first-token→end window because local servers often flush generated
    // tokens in one batch, collapsing that window to ~0 and inflating the rate.
    let elapsed = ended.duration_since(started).as_secs_f64();
    let tokens_per_sec = match completion_tokens {
        Some(tokens) if tokens > 0 && elapsed > 0.0 => Some(tokens as f64 / elapsed),
        _ => None,
    };

    let _ = session.events.send(AgentEvent::Metrics(Metrics {
        prompt_tokens: usage.as_ref().map(|u| u.prompt_tokens),
        completion_tokens,
        total_tokens: usage.as_ref().map(|u| u.total_tokens),
        context_window: session.context_window,
        ttft_ms,
        tokens_per_sec,
    }));
}

async fn emit_error(session: &Session, message: String) {
    *session.status.lock().await = SessionStatus::Idle;
    let _ = session.events.send(AgentEvent::Error { message });
}
