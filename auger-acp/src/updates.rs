//! Maps auger records and stream events to ACP `session/update` payloads.

use crate::jsonrpc::{self, Out};
use auger_core::{RecordBody, TurnsChange};
use auger_driver::{InputEntry, Turn};
use auger_toolengine::{Decision, ToolCallResult, ToolData, ToolFact, ToolOutcome};
use provider::{Block, BlockKind, StreamEvent, ToolCallRequest};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) struct Mapper {
    cwd: PathBuf,
    /// Kind of each block of the stream in flight, by index.
    blocks: HashMap<usize, BlockKind>,
}

impl Mapper {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            blocks: HashMap::new(),
        }
    }

    /// Text and reasoning deltas of a live stream.
    pub(crate) fn stream_event(&mut self, event: &StreamEvent) -> Option<Value> {
        match event {
            StreamEvent::BlockStart { index, kind } => {
                self.blocks.insert(*index, kind.clone());
                None
            }
            StreamEvent::BlockDelta { index, delta } => match self.blocks.get(index)? {
                BlockKind::Text => Some(chunk("agent_message_chunk", delta)),
                BlockKind::Reasoning => Some(chunk("agent_thought_chunk", delta)),
                BlockKind::ToolCall { .. } => None,
            },
            _ => None,
        }
    }

    /// Updates for a record. Replay also sends the user's messages and the
    /// model's text, which a live turn already sent as stream deltas.
    pub(crate) fn record(&mut self, body: &RecordBody, replay: bool) -> Vec<Value> {
        match body {
            RecordBody::Turns(TurnsChange::PushTurn(Turn::Input { entries })) if replay => {
                entries.iter().filter_map(user_chunk).collect()
            }
            RecordBody::Turns(TurnsChange::PushEntry(entry)) if replay => {
                user_chunk(entry).into_iter().collect()
            }
            RecordBody::Turns(TurnsChange::PushTurn(Turn::Output(response))) => {
                let mut updates = Vec::new();
                for block in response.blocks() {
                    match block {
                        Block::Text(text) if replay => {
                            updates.push(chunk("agent_message_chunk", text))
                        }
                        Block::Reasoning { text } if replay => {
                            updates.push(chunk("agent_thought_chunk", text))
                        }
                        Block::ToolCall(call) => {
                            let mut update = self.call_fields(call);
                            update.insert("sessionUpdate".into(), json!("tool_call"));
                            updates.push(Value::Object(update));
                        }
                        _ => {}
                    }
                }
                updates
            }
            RecordBody::Tool(fact) => self.tool_fact(fact),
            _ => Vec::new(),
        }
    }

    /// The fields of a tool call, shared by `tool_call` and permission requests.
    pub(crate) fn call_fields(&self, call: &ToolCallRequest) -> Map<String, Value> {
        let args: Value = serde_json::from_str(&call.arguments)
            .unwrap_or_else(|_| Value::String(call.arguments.clone()));
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .map(|path| self.cwd.join(path));
        let detail = args
            .get("path")
            .or_else(|| args.get("command"))
            .and_then(Value::as_str);
        let title = match detail {
            Some(detail) => format!("{} {detail}", call.name),
            None => call.name.clone(),
        };
        let mut fields = Map::new();
        fields.insert("toolCallId".into(), json!(call.id));
        fields.insert("title".into(), json!(title));
        fields.insert("name".into(), json!(call.name));
        fields.insert("kind".into(), json!(kind_of(&call.name)));
        fields.insert("status".into(), json!("pending"));
        fields.insert("rawInput".into(), args);
        if let Some(path) = path {
            fields.insert(
                "locations".into(),
                json!([{"path": path.to_string_lossy()}]),
            );
        }
        fields
    }

    fn tool_fact(&mut self, fact: &ToolFact) -> Vec<Value> {
        match fact {
            ToolFact::Decided {
                tool_call_id,
                decision: Decision::Denied { reason, .. },
            } => {
                let text = reason.clone().unwrap_or_else(|| "Denied".to_string());
                vec![tool_update(tool_call_id, "failed", Some(&text))]
            }
            ToolFact::Decided { .. } => Vec::new(),
            ToolFact::Started { tool_call_id, .. } => {
                vec![tool_update(tool_call_id, "in_progress", None)]
            }
            ToolFact::Finished(result) => vec![finished(result)],
            ToolFact::CwdChanged { cwd } => {
                self.cwd = cwd.clone();
                Vec::new()
            }
        }
    }
}

/// Send one update for `session_id`.
pub(crate) fn send(out: &Out, session_id: &str, update: Value) {
    out.send(&jsonrpc::notification(
        "session/update",
        json!({"sessionId": session_id, "update": update}),
    ));
}

fn chunk(kind: &str, text: &str) -> Value {
    json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}})
}

fn user_chunk(entry: &InputEntry) -> Option<Value> {
    match entry {
        InputEntry::User(prompt) => Some(chunk("user_message_chunk", &prompt.message)),
        InputEntry::Harness(_) => None,
    }
}

fn kind_of(tool: &str) -> &'static str {
    match tool {
        "read_file" => "read",
        "edit_file" | "write_file" => "edit",
        "shell" => "execute",
        _ => "other",
    }
}

fn tool_update(tool_call_id: &str, status: &str, text: Option<&str>) -> Value {
    let mut update = json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": tool_call_id,
        "status": status,
    });
    if let Some(text) = text {
        update["content"] = json!([{"type": "content", "content": {"type": "text", "text": text}}]);
    }
    update
}

fn finished(result: &ToolCallResult) -> Value {
    let id = String::from(result.tool_call_id());
    let (status, text) = match result.outcome() {
        ToolOutcome::Success { content } => ("completed", data_text(content)),
        ToolOutcome::Error { error } => ("failed", data_text(error)),
        ToolOutcome::Denied { reason } => (
            "failed",
            reason.clone().unwrap_or_else(|| "Denied".to_string()),
        ),
        ToolOutcome::Interrupted => ("failed", "Interrupted".to_string()),
    };
    tool_update(&id, status, Some(&text))
}

fn data_text(data: &[ToolData]) -> String {
    data.iter()
        .map(|d| match d {
            ToolData::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
