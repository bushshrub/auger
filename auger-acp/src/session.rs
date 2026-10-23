//! One auger session as ACP sees it: the prompt in flight and the permission
//! requests waiting on the client.

use crate::jsonrpc::{self, Out, PROMPT_FAILED, RpcError};
use crate::updates::{self, Mapper};
use auger_core::{RecordBody, SessionCommand, SessionEvent, SessionHandle, StreamRecord, TurnsChange};
use auger_driver::{Prompt, Turn};
use provider::{ToolCallRequest, UserPrompt};
use serde_json::{Value, json};
use std::collections::HashMap;

const ALLOW: &str = "allow-once";
const REJECT: &str = "reject-once";

pub(crate) struct Session {
    id: String,
    handle: SessionHandle,
    mapper: Mapper,
    active: Option<Active>,
    /// Permission requests awaiting the client, by request id.
    consents: HashMap<i64, String>,
}

struct Active {
    request_id: Value,
    cancelled: bool,
    had_tool_calls: bool,
}

/// How a prompt turn ends.
enum Outcome {
    Stop(&'static str),
    Failed(String),
}

impl Session {
    pub(crate) fn new(id: String, handle: SessionHandle, mapper: Mapper) -> Self {
        Self {
            id,
            handle,
            mapper,
            active: None,
            consents: HashMap::new(),
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn start_prompt(&mut self, request_id: Value, text: String) -> Result<(), RpcError> {
        if self.active.is_some() {
            return Err(RpcError::invalid("a prompt is already running in this session"));
        }
        self.handle
            .send_command(SessionCommand::SendMessage(Prompt::User(UserPrompt::new(text))))
            .map_err(|()| RpcError::internal("session has stopped"))?;
        self.active = Some(Active {
            request_id,
            cancelled: false,
            had_tool_calls: false,
        });
        Ok(())
    }

    /// `session/cancel`: interrupt the stream and deny waiting permissions.
    /// The prompt then answers `cancelled` once the session settles.
    pub(crate) fn cancel(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        active.cancelled = true;
        let _ = self.handle.send_command(SessionCommand::Interrupt);
        self.deny_all();
    }

    /// Deny every permission request still waiting, so the tool phase can end.
    pub(crate) fn deny_all(&mut self) {
        for (_, tool_call_id) in self.consents.drain() {
            let _ = self.handle.send_command(SessionCommand::ToolDecision {
                id: tool_call_id,
                approved: false,
                message: None,
            });
        }
    }

    /// Apply the client's reply to permission request `request_id`. Returns
    /// false if the request is not one of ours.
    pub(crate) fn answer_permission(&mut self, request_id: i64, reply: &Value) -> bool {
        let Some(tool_call_id) = self.consents.remove(&request_id) else {
            return false;
        };
        let approved = reply.pointer("/outcome/outcome").and_then(Value::as_str)
            == Some("selected")
            && reply.pointer("/outcome/optionId").and_then(Value::as_str) == Some(ALLOW);
        let _ = self.handle.send_command(SessionCommand::ToolDecision {
            id: tool_call_id,
            approved,
            message: None,
        });
        true
    }

    pub(crate) fn on_event(&mut self, event: SessionEvent, out: &Out) {
        match event {
            SessionEvent::StreamEvent(event) => {
                if let Some(update) = self.mapper.stream_event(&event) {
                    updates::send(out, &self.id, update);
                }
            }
            SessionEvent::Record(record) => self.on_record(&record.body, out),
            SessionEvent::ToolConsentRequired { tool_calls } => {
                for call in &tool_calls {
                    self.ask_permission(call, out);
                }
            }
            SessionEvent::ToolResultsWithheld => self.settle(out, Outcome::Stop("end_turn")),
            SessionEvent::ToolsMissing { names } => {
                tracing::warn!(?names, "restored session used tools that are no longer available");
            }
            SessionEvent::Closed => self.settle(out, Outcome::Failed("session closed".into())),
        }
    }

    fn on_record(&mut self, body: &RecordBody, out: &Out) {
        for update in self.mapper.record(body, false) {
            updates::send(out, &self.id, update);
        }
        match body {
            RecordBody::Turns(TurnsChange::PushTurn(Turn::Output(response))) => {
                if let Some(active) = self.active.as_mut() {
                    active.had_tool_calls = !response.tool_calls().is_empty();
                }
            }
            RecordBody::Stream(StreamRecord::Started) => {
                // A cancel that arrived during the tool phase lands here.
                if self.active.as_ref().is_some_and(|active| active.cancelled) {
                    let _ = self.handle.send_command(SessionCommand::Interrupt);
                }
            }
            RecordBody::Stream(StreamRecord::Completed { stop_reason, .. }) => {
                let had_tool_calls = self.active.as_ref().is_some_and(|a| a.had_tool_calls);
                if !had_tool_calls {
                    self.settle(out, Outcome::Stop(stop_reason_of(stop_reason.as_deref())));
                }
            }
            RecordBody::Stream(StreamRecord::Interrupted { .. }) => {
                self.settle(out, Outcome::Stop("cancelled"))
            }
            RecordBody::Stream(StreamRecord::Failed { error, .. }) => {
                self.settle(out, Outcome::Failed(error.to_string()))
            }
            _ => {}
        }
    }

    fn ask_permission(&mut self, call: &ToolCallRequest, out: &Out) {
        if self.active.as_ref().is_some_and(|active| active.cancelled) {
            let _ = self.handle.send_command(SessionCommand::ToolDecision {
                id: call.id.clone(),
                approved: false,
                message: None,
            });
            return;
        }
        let request_id = out.request_id();
        self.consents.insert(request_id, call.id.clone());
        let params = json!({
            "sessionId": self.id,
            "toolCall": Value::Object(self.mapper.call_fields(call)),
            "options": [
                {"optionId": ALLOW, "name": "Allow", "kind": "allow_once"},
                {"optionId": REJECT, "name": "Reject", "kind": "reject_once"},
            ],
        });
        out.send(&jsonrpc::request(request_id, "session/request_permission", params));
    }

    /// End the prompt in flight, if any. A cancelled prompt always reports
    /// `cancelled`, never an error.
    fn settle(&mut self, out: &Out, outcome: Outcome) {
        let Some(active) = self.active.take() else {
            return;
        };
        let reply = match outcome {
            Outcome::Stop(reason) => {
                let reason = if active.cancelled { "cancelled" } else { reason };
                jsonrpc::result(&active.request_id, json!({"stopReason": reason}))
            }
            Outcome::Failed(_) if active.cancelled => {
                jsonrpc::result(&active.request_id, json!({"stopReason": "cancelled"}))
            }
            Outcome::Failed(message) => {
                jsonrpc::error(&active.request_id, &RpcError::new(PROMPT_FAILED, message))
            }
        };
        out.send(&reply);
    }

    pub(crate) fn stop(&self) {
        let (reply_tx, _reply_rx) = std::sync::mpsc::channel();
        let _ = self.handle.send_command(SessionCommand::Stop { reply_tx });
    }
}

/// Map the provider's stop reason to an ACP stop reason.
fn stop_reason_of(provider_reason: Option<&str>) -> &'static str {
    match provider_reason {
        Some("max_tokens") | Some("length") => "max_tokens",
        _ => "end_turn",
    }
}
