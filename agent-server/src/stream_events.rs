//! Translation from session events into the events sent to clients.
//!
//! The provider streams indexed blocks, while clients consume flat deltas. A
//! delta does not say what kind of block it belongs to, so the open blocks are
//! tracked as their starts arrive.

use auger_core::SessionEvent;
use provider::BlockKind;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;

enum Block {
    Text,
    Reasoning,
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
}

/// The open blocks for one subscriber's view of the stream.
#[derive(Default)]
pub(crate) struct BlockTracker {
    blocks: HashMap<usize, Block>,
}

impl BlockTracker {
    /// Translate a session event into client events. May be empty: a block
    /// start carries nothing the client can show yet.
    pub(crate) fn translate(&mut self, event: SessionEvent) -> Vec<Value> {
        match event {
            SessionEvent::StreamEvent(event) => self.translate_stream(event),
            SessionEvent::ToolConsentRequired { tool_calls } => {
                vec![json!({ "type": "tool_consent_required", "tool_calls": tool_calls })]
            }
            SessionEvent::ToolCallResult(result) => vec![json!({
                "type": "tool_call_result",
                "id": result.tool_call_id(),
                "result": result,
            })],
            SessionEvent::TurnComplete { usage, stop_reason } => vec![json!({
                "type": "done",
                "usage": usage,
                "stop_reason": stop_reason,
            })],
            SessionEvent::Interrupted => vec![json!({ "type": "interrupted" })],
            SessionEvent::StreamError { error } => {
                vec![json!({ "type": "stream_error", "error": error })]
            }
            SessionEvent::Closed => vec![json!({ "type": "closed" })],
        }
    }

    fn translate_stream(&mut self, event: provider::StreamEvent) -> Vec<Value> {
        match event {
            provider::StreamEvent::BlockStart { index, kind } => {
                let block = match kind {
                    BlockKind::Text => Block::Text,
                    BlockKind::Reasoning => Block::Reasoning,
                    BlockKind::ToolCall { id, name } => Block::ToolCall {
                        id,
                        name,
                        arguments: String::new(),
                    },
                };
                self.blocks.insert(index, block);
                Vec::new()
            }
            provider::StreamEvent::BlockDelta { index, delta } => {
                match self.blocks.get_mut(&index) {
                    Some(Block::Text) => vec![json!({ "type": "text_delta", "text": delta })],
                    Some(Block::Reasoning) => {
                        vec![json!({ "type": "reasoning_delta", "text": delta })]
                    }
                    Some(Block::ToolCall {
                        id,
                        name,
                        arguments,
                    }) => {
                        arguments.push_str(&delta);
                        vec![json!({
                            "type": "tool_call",
                            "id": id,
                            "name": name,
                            "arguments": delta,
                        })]
                    }
                    None => Vec::new(),
                }
            }
            // Only tool calls report completion; the client keeps whatever text
            // and reasoning deltas it already assembled.
            provider::StreamEvent::BlockEnd { index } => match self.blocks.remove(&index) {
                Some(Block::ToolCall {
                    id,
                    name,
                    arguments,
                }) => vec![json!({
                    "type": "tool_call_complete",
                    "id": id,
                    "name": name,
                    "arguments": arguments,
                })],
                _ => Vec::new(),
            },
            // Usage is folded into the turn's `done`, which auger-core emits
            // once the turn settles.
            provider::StreamEvent::Usage(_) => Vec::new(),
        }
    }
}
