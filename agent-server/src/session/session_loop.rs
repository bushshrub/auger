use std::collections::HashMap;
use std::sync::Arc;
use provider::{LlmProvider, ToolCall};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;
use crate::conversation::Conversation;
use crate::session::events::{Cmd, SessionEvent};
use crate::session::handle::SessionHandle;
use crate::session::{SessionId, SessionStatus};
use crate::system_prompt::SystemPrompt;
use futures::stream::StreamExt;
use tracing::{debug, error, info, trace};
use agent_tools::{Dummy, Tool};
use crate::conversation::UserContent;

/// Represents a conversation between the user and a clanker.
pub(crate) struct Session {
    id: SessionId,
    model: String,
    conversation: Conversation,
    status: SessionStatus,
    provider: Arc<dyn LlmProvider>,
    events: broadcast::Sender<SessionEvent>,
}

impl Session {
    // TODO: no need to share LlmProvider. session should take care of spawning it.
    pub fn spawn(prompt: SystemPrompt, provider: &Arc<impl LlmProvider + 'static>, model: String) -> SessionHandle {
        let (cmds_tx, cmds_rx) = mpsc::channel(32);
        let (events_tx, _) = broadcast::channel(32);

        let id = Uuid::new_v4();

        let session = Session {
            id,
            model: model.clone(),
            conversation: Conversation::new(prompt.into()),
            status: SessionStatus::Idle,
            provider: provider.clone(),
            events: events_tx.clone(),
        };

        tokio::spawn(session.run(cmds_rx));
        SessionHandle::new(id, model, cmds_tx, events_tx)
    }

    /// Runs the session. The user will send commands via `rx`.
    async fn run(self, mut rx: mpsc::Receiver<Cmd>) {

        let mut tool_calls: HashMap<String, (String, String)> = HashMap::new();

        info!(session_id = %self.id, "Starting session");
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::SendMessage(content) => {
                    let _ = self.events.send(content.clone().into());

                    // TODO: these can just be sent off as a vec anyway...
                    let user_text = content.iter()
                        .filter_map(|c| match c {
                            UserContent::Text(t) => Some(t.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let dummy = Dummy;
                    let dummy_details = dummy.details();
                    let dummy_params = dummy.parameters();
                    let request = provider::LlmRequest {
                        model: self.model.clone(),
                        messages: vec![provider::Message::User(user_text)],
                        tools: vec![provider::ToolDefinition {
                            name: dummy_details.name.to_string(),
                            description: Some(dummy_details.description.to_string()),
                            parameters: dummy_params.0,
                        }],
                    };
                    if let Ok(mut stream) = self.provider.stream(request).await {

                        while let Some(event_result) = stream.next().await {
                            match event_result {
                                Ok(provider::StreamEvent::TextDelta(text)) => {
                                    trace!("text delta: {}", text);
                                    let _ = self.events.send(SessionEvent::Content { delta: text });
                                }
                                Ok(provider::StreamEvent::ReasoningDelta(text)) => {
                                    trace!("reasoning delta: {}", text);
                                    let _ = self.events.send(SessionEvent::Reasoning { delta: text });
                                }
                                Ok(provider::StreamEvent::ToolCall { id, name, arguments }) => {
                                    trace!(tool_call_id = %id, tool = %name, "tool call delta: {}", arguments)
                                    // TODO: handle tool call deltas
                                }
                                // clanker has finished generating tool call request
                                Ok(provider::StreamEvent::ToolCallComplete {id, name, arguments}) => {
                                    debug!(tool_call_id = %id, tool = %name, "tool call complete: {}", arguments);
                                    tool_calls.insert(id.clone(), (name.clone(), arguments.clone()));
                                    let _ = self.events.send(SessionEvent::ToolCallRequest { id, name, arguments });

                                }
                                Ok(provider::StreamEvent::Done { usage, stop_reason }) => {
                                    debug!("Response has finished generating. Usage: {:?}, stop reason: {:?}", usage, stop_reason);
                                    let _ = self.events.send(SessionEvent::Done);
                                },
                                // TODO: Handle errors while streaming e.g. rate limit, connection drops.
                                Err(e) => {
                                    error!("Error while streaming response from provider: {:?}", e);
                                },
                            }
                        }
                    }
                }
                Cmd::ApproveToolCall { tool_call_id } => {
                    let (tool_name, tool_args) = match tool_calls.get(&tool_call_id) {
                        Some((name, args)) => (name.clone(), args.clone()),
                        None => {
                            error!(tool_call_id = %tool_call_id, "No tool call found with this ID");
                            continue;
                        }
                    };
                    debug!(tool_call_id = %tool_call_id, "Tool call approved by user");
                }
                Cmd::DenyToolCall { tool_call_id } => {
                    debug!(tool_call_id = %tool_call_id, "Tool call denied by user");
                }
            }
        }
        info!(session_id = %self.id, "Session has been closed");
    }
}