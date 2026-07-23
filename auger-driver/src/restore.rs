//! Restore support for auger-driver sessions.

use crate::LlmStreamingFailed;
use crate::LlmStreamingInterrupted;
use crate::TypedAgent;
use crate::WaitingForToolResponses;
use crate::WaitingForUserMessage;
use provider::LlmError;
use provider::LlmModel;
use provider::Message;
use provider::StreamEvent;
use provider::ToolDefinition;
use serde::Deserialize;
use serde::Serialize;

/// Driver state reconstructed by the persistence owner.
#[derive(Debug, Serialize, Deserialize)]
pub enum RestoreState {
    WaitingForUserMessage {
        messages: Vec<Message>,
    },
    WaitingForToolResponses {
        messages: Vec<Message>,
    },
    Interrupted {
        messages: Vec<Message>,
        events: Vec<StreamEvent>,
    },
    Failed {
        messages: Vec<Message>,
        events: Vec<StreamEvent>,
        error: LlmError,
    },
}

impl RestoreState {
    pub fn from_messages(messages: Vec<Message>) -> Self {
        let waiting_for_tools = matches!(
            messages.last(),
            Some(Message::Assistant { response }) if !response.tool_calls.is_empty()
        );
        if waiting_for_tools {
            Self::WaitingForToolResponses { messages }
        } else {
            Self::WaitingForUserMessage { messages }
        }
    }
}

/// An agent restored from persistent state.
pub enum RestoredAgent {
    WaitingForUserMessage(TypedAgent<WaitingForUserMessage>),
    WaitingForToolResponses(TypedAgent<WaitingForToolResponses>),
    Interrupted(TypedAgent<LlmStreamingInterrupted>),
    Failed(TypedAgent<LlmStreamingFailed>),
}

/// Restore an agent into the state selected by the persistence owner.
pub fn restore(model: LlmModel, tools: Vec<ToolDefinition>, state: RestoreState) -> RestoredAgent {
    macro_rules! agent {
        ($messages:expr, $state:expr) => {
            TypedAgent {
                model,
                messages: $messages,
                tools,
                state: $state,
            }
        };
    }

    match state {
        RestoreState::WaitingForUserMessage { messages } => {
            RestoredAgent::WaitingForUserMessage(agent!(messages, WaitingForUserMessage))
        }
        RestoreState::WaitingForToolResponses { messages } => {
            RestoredAgent::WaitingForToolResponses(agent!(messages, WaitingForToolResponses))
        }
        RestoreState::Interrupted { messages, events } => {
            RestoredAgent::Interrupted(agent!(messages, LlmStreamingInterrupted::new(events)))
        }
        RestoreState::Failed {
            messages,
            events,
            error,
        } => RestoredAgent::Failed(agent!(messages, LlmStreamingFailed::new(events, error))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::AssistantResponse;
    use provider::ToolCallRequest;

    #[test]
    fn messages_with_outstanding_tool_calls_wait_for_tool_responses() {
        let messages = vec![Message::Assistant {
            response: AssistantResponse {
                reasoning: None,
                content: String::new(),
                tool_calls: vec![ToolCallRequest {
                    id: "call-1".to_string(),
                    name: "shell".to_string(),
                    arguments: "{}".to_string(),
                }],
            },
        }];

        assert!(matches!(
            RestoreState::from_messages(messages),
            RestoreState::WaitingForToolResponses { .. }
        ));
    }

    #[test]
    fn completed_messages_wait_for_user_input() {
        let messages = vec![Message::Assistant {
            response: AssistantResponse {
                reasoning: None,
                content: String::new(),
                tool_calls: vec![],
            },
        }];

        assert!(matches!(
            RestoreState::from_messages(messages),
            RestoreState::WaitingForUserMessage { .. }
        ));
    }
}
