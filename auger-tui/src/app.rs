use crate::types::AppEvent;
use crate::types::ChatItem;
use crate::types::SessionInfo;
use crate::types::SseEvent;
use crate::types::Status;
use crate::types::ToolDecision;
use ratatui::widgets::ListState;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    SessionList,
    Chat,
}

pub struct App {
    pub view: View,
    pub should_quit: bool,

    // Session list
    pub sessions: Vec<SessionInfo>,
    pub session_list_state: ListState,

    // Chat
    pub session_id: Option<Uuid>,
    pub write_token: Option<String>,
    pub read_token: Option<String>,
    pub ctx_window: u64,
    pub ctx_used: u64,
    pub items: Vec<ChatItem>,
    pub pending_tool_id: Option<String>,
    pub status: Status,
    pub input: String,
    /// Lines above the bottom that the user has scrolled. 0 = pinned to bottom.
    pub scroll_from_bottom: u16,

    // In-progress streaming indices
    pub assistant_idx: Option<usize>,
    pub reasoning_idx: Option<usize>,

    pub error_msg: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let mut session_list_state = ListState::default();
        session_list_state.select(Some(0));
        Self {
            view: View::SessionList,
            should_quit: false,
            sessions: vec![],
            session_list_state,
            session_id: None,
            write_token: None,
            read_token: None,
            ctx_window: 0,
            ctx_used: 0,
            items: vec![],
            pending_tool_id: None,
            status: Status::Connecting,
            input: String::new(),
            scroll_from_bottom: 0,
            assistant_idx: None,
            reasoning_idx: None,
            error_msg: None,
        }
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::SessionsLoaded(sessions) => {
                self.sessions = sessions;
                if self.sessions.is_empty() {
                    self.session_list_state.select(None);
                } else {
                    self.session_list_state.select(Some(0));
                }
            }

            AppEvent::SessionCreated {
                session_id,
                write_token,
                read_token,
                context_window,
            } => {
                self.session_id = Some(session_id);
                self.write_token = Some(write_token);
                self.read_token = Some(read_token);
                self.ctx_window = context_window;
                self.ctx_used = 0;
                self.items.clear();
                self.pending_tool_id = None;
                self.status = Status::Idle;
                self.assistant_idx = None;
                self.reasoning_idx = None;
                self.view = View::Chat;
            }

            AppEvent::SnapshotLines(lines) => {
                self.apply_snapshot_lines(&lines);
                // Only go idle if there's no pending tool waiting for approval.
                if self.pending_tool_id.is_none() {
                    self.status = Status::Idle;
                }
            }

            AppEvent::Sse(sse) => self.handle_sse(sse),

            AppEvent::NetworkError(msg) => {
                self.error_msg = Some(msg);
                self.status = Status::Idle;
            }
        }
    }

    fn handle_sse(&mut self, ev: SseEvent) {
        match ev {
            SseEvent::Content { text } => {
                self.reasoning_idx = None;
                match self.assistant_idx {
                    Some(i) => {
                        if let Some(ChatItem::Assistant { text: t }) = self.items.get_mut(i) {
                            t.push_str(&text);
                        }
                    }
                    None => {
                        self.items.push(ChatItem::Assistant { text });
                        self.assistant_idx = Some(self.items.len() - 1);
                    }
                }
            }

            SseEvent::Reasoning { text } => {
                self.assistant_idx = None;
                match self.reasoning_idx {
                    Some(i) => {
                        if let Some(ChatItem::Reasoning { text: t, .. }) = self.items.get_mut(i) {
                            t.push_str(&text);
                        }
                    }
                    None => {
                        self.items.push(ChatItem::Reasoning {
                            text,
                            collapsed: true,
                        });
                        self.reasoning_idx = Some(self.items.len() - 1);
                    }
                }
            }

            SseEvent::ToolCall {
                id,
                name,
                arguments,
            } => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                self.pending_tool_id = Some(id.clone());
                self.items.push(ChatItem::Tool {
                    id,
                    name,
                    args: arguments,
                    result: None,
                    decision: None,
                });
                self.status = Status::Running;
            }

            SseEvent::ToolCallComplete {
                id: _,
            } => {
                // Tool finished executing; the result will arrive separately.
                self.assistant_idx = None;
            }

            SseEvent::Interrupted => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                self.items
                    .push(ChatItem::Error { text: "Interrupted".into() });
                self.status = Status::Idle;
            }

            SseEvent::StreamClosed => {
                self.status = Status::Idle;
            }

            SseEvent::ToolResult { id, content } => {
                self.assistant_idx = None;
                if let Some(item) = self
                    .items
                    .iter_mut()
                    .find(|i| matches!(i, ChatItem::Tool { id: tid, .. } if tid == &id))
                {
                    if let ChatItem::Tool {
                        result, decision, ..
                    } = item
                    {
                        *result = Some(content);
                        if decision.is_none() {
                            *decision = Some(ToolDecision::Approved);
                        }
                    }
                }
                if self.pending_tool_id.as_deref() == Some(&id) {
                    self.pending_tool_id = None;
                }
            }

            SseEvent::Metrics {
                prompt_tokens,
                completion_tokens,
                total_tokens,
            } => {
                self.ctx_used = total_tokens
                    .or_else(|| prompt_tokens.zip(completion_tokens).map(|(p, c)| p + c))
                    .unwrap_or(self.ctx_used);
            }

            SseEvent::TurnComplete => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                self.status = Status::Idle;
            }

            SseEvent::StreamError { message } => {
                self.items.push(ChatItem::Error { text: message });
                self.status = Status::Idle;
            }
        }
        // Only auto-follow the bottom if the user hasn't manually scrolled up.
        if self.scroll_from_bottom == 0 {
            self.scroll_to_bottom();
        }
    }

    pub fn open_session(&mut self, info: &SessionInfo) {
        self.session_id = Some(info.session_id);
        self.write_token = Some(info.write_token.clone());
        self.read_token = Some(info.read_token.clone());
        self.ctx_window = info.context_window;
        self.ctx_used = 0;
        self.items.clear();
        self.pending_tool_id = None;
        self.status = Status::Connecting;
        self.assistant_idx = None;
        self.reasoning_idx = None;
        self.scroll_from_bottom = 0;
        self.view = View::Chat;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }

    pub fn scroll_up(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(lines);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(lines);
    }

    pub fn session_list_next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = self.session_list_state.selected().unwrap_or(0);
        self.session_list_state
            .select(Some((i + 1) % self.sessions.len()));
    }

    pub fn session_list_prev(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = self.session_list_state.selected().unwrap_or(0);
        self.session_list_state.select(Some(if i == 0 {
            self.sessions.len() - 1
        } else {
            i - 1
        }));
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.session_list_state
            .selected()
            .and_then(|i| self.sessions.get(i))
    }

    pub fn send_message(&mut self) -> Option<(Uuid, String, String)> {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.status != Status::Idle {
            return None;
        }
        let session_id = self.session_id?;
        let write_token = self.write_token.clone()?;
        self.items.push(ChatItem::User { text: text.clone() });
        self.input.clear();
        self.status = Status::Running;
        self.assistant_idx = None;
        self.reasoning_idx = None;
        self.scroll_to_bottom();
        Some((session_id, write_token, text))
    }

    /// Reconstruct chat items from NDJSON snapshot lines produced by the
    /// server's TraceWriter.  The trace is flat JSONL — each line has a
    /// `"kind"` discriminator (`"session"`, `"turn"`, `"event"`) with the
    /// actual payload nested under `"turn"` or `"event"`.
    pub fn apply_snapshot_lines(&mut self, lines: &[String]) {
        self.items.clear();
        self.assistant_idx = None;
        self.reasoning_idx = None;

        let mut tool_idx_map: HashMap<String, usize> = HashMap::new();
        let mut last_block_ids: HashSet<String> = HashSet::new();

        for line in lines {
            let obj: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // ── Session header ─────────────────────────────────────────
            if obj.get("kind").and_then(|k| k.as_str()) == Some("session") {
                continue;
            }

            // ── Turn record ────────────────────────────────────────────
            if obj.get("kind").and_then(|k| k.as_str()) == Some("turn") {
                let turn = obj.get("turn");
                if let Some(turn) = turn.and_then(|t| t.as_object()) {
                    // User message turn: {"input_message": {"content": [...]}}
                    // Content can contain "text" (user message) and "tool_result" (folded results)
                    if let Some(input_msg) = turn.get("input_message").and_then(|m| m.as_object()) {
                        last_block_ids.clear();
                        if let Some(content) = input_msg.get("content").and_then(|c| c.as_array()) {
                            for item in content {
                                match item.get("type").and_then(|t| t.as_str()) {
                                    Some("text") => {
                                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                            self.items.push(ChatItem::User {
                                                text: text.to_string(),
                                            });
                                        }
                                    }
                                    Some("tool_result") => {
                                        if let (Some(tool_call_id), Some(result_content)) = (
                                            item.get("tool_call_id").and_then(|i| i.as_str()),
                                            item.get("content").and_then(|c| c.as_array()),
                                        ) {
                                            let text = result_content.iter()
                                                .filter_map(|c| {
                                                    // Could be {"text": "..." } or {"text": {"text": "..."}}
                                                    if let Some(inner) = c.get("text") {
                                                        inner.as_str().or_else(|| {
                                                            inner.get("text").and_then(|t| t.as_str())
                                                        })
                                                    } else {
                                                        None
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .join("");
                                            if let Some(idx) = self.items.iter().position(|i| {
                                                matches!(i, ChatItem::Tool { id, .. } if id == tool_call_id)
                                            }) {
                                                if let ChatItem::Tool { result, .. } = &mut self.items[idx] {
                                                    *result = Some(text);
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        continue;
                    }

                    // Assistant message turn: {"assistant_message": {"outcome": {...}}}
                    if let Some(assist_msg) = turn.get("assistant_message").and_then(|m| m.as_object()) {
                        last_block_ids.clear();
                        if let Some(outcome) = assist_msg.get("outcome").and_then(|o| o.as_object()) {
                            // outcome: {"completed": {"response": {...}}} or {"interrupted": {...}}
                            if let Some(payload) = outcome
                                .get("completed")
                                .or_else(|| outcome.get("interrupted"))
                                .and_then(|v| v.as_object())
                            {
                                let response = payload
                                    .get("response")
                                    .and_then(|r| r.as_object())
                                    .unwrap_or(payload);

                                // Reasoning
                                if let Some(reasoning) =
                                    response.get("reasoning").and_then(|r| r.as_str())
                                {
                                    if !reasoning.is_empty() {
                                        self.items.push(ChatItem::Reasoning {
                                            text: reasoning.to_string(),
                                            collapsed: true,
                                        });
                                    }
                                }
                                // Content
                                if let Some(content) =
                                    response.get("content").and_then(|c| c.as_str())
                                {
                                    if !content.is_empty() {
                                        self.items.push(ChatItem::Assistant {
                                            text: content.to_string(),
                                        });
                                    }
                                }
                                // Tool calls
                                if let Some(tool_calls) =
                                    response.get("tool_calls").and_then(|t| t.as_array())
                                {
                                    for tc in tool_calls {
                                        if let (Some(id), Some(name), Some(args)) = (
                                            tc.get("id").and_then(|i| i.as_str()),
                                            tc.get("name").and_then(|n| n.as_str()),
                                            tc.get("arguments"),
                                        ) {
                                            let args_str = if args.is_string() {
                                                args.as_str().unwrap_or("").to_string()
                                            } else {
                                                args.to_string()
                                            };
                                            let idx = self.items.len();
                                            tool_idx_map.insert(id.to_string(), idx);
                                            last_block_ids.insert(id.to_string());
                                            self.items.push(ChatItem::Tool {
                                                id: id.to_string(),
                                                name: name.to_string(),
                                                args: args_str,
                                                result: None,
                                                decision: None,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }

            // ── Event record ───────────────────────────────────────────
            // Events have nested structure: {"event": {"tool_call_requested": {...}}}
            // or {"event": {"tool_call_result": {...}}}
            if obj.get("kind").and_then(|k| k.as_str()) == Some("event") {
                let event = obj.get("event");
                if let Some(event) = event.and_then(|e| e.as_object()) {
                    // tool_call_result event
                    if let Some(result_ev) = event.get("tool_call_result").and_then(|r| r.as_object()) {
                        last_block_ids.clear();
                        if let (Some(tool_call_id), Some(outcome)) = (
                            result_ev.get("tool_call_id").and_then(|i| i.as_str()),
                            result_ev.get("outcome").and_then(|o| o.as_object()),
                        ) {
                            let content = if let Some(contents) = outcome.get("content").and_then(|c| c.as_array()) {
                                contents.iter()
                                    .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("")
                            } else if let Some(error_arr) = outcome.get("error").and_then(|e| e.as_array()) {
                                error_arr.iter()
                                    .filter_map(|e| e.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("")
                            } else {
                                String::new()
                            };
                            if let Some(item) = self.items.iter_mut().find(|i| {
                                matches!(i, ChatItem::Tool { id, .. } if id == tool_call_id)
                            }) {
                                if let ChatItem::Tool { result, .. } = item {
                                    *result = Some(content);
                                }
                            }
                        }
                    }
                    // tool_authorization event
                    else if let Some(auth_ev) = event.get("tool_authorization").and_then(|a| a.as_object()) {
                        if let (Some(tool_call_id), Some(decision)) = (
                            auth_ev.get("tool_call_id").and_then(|i| i.as_str()),
                            auth_ev.get("decision").and_then(|d| d.as_str()),
                        ) {
                            if let Some(item) = self.items.iter_mut().find(|i| {
                                matches!(i, ChatItem::Tool { id, .. } if id == tool_call_id)
                            }) {
                                if let ChatItem::Tool { decision: dec, .. } = item {
                                    if dec.is_none() {
                                        *dec = Some(if decision == "approved" {
                                            ToolDecision::Approved
                                        } else {
                                            ToolDecision::Denied
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }
        }

        // Mark pending tool calls (from the last assistant block with no result).
        for (id, idx) in &tool_idx_map {
            if last_block_ids.contains(id) {
                self.pending_tool_id = Some(id.clone());
            } else if let Some(ChatItem::Tool { decision, .. }) = self.items.get_mut(*idx) {
                if decision.is_none() {
                    *decision = Some(ToolDecision::Approved);
                }
            }
        }

        self.scroll_to_bottom();
    }

    /// Returns (session_id, write_token, tool_call_id) if there's a pending
    /// tool.
    pub fn approve_tool(&mut self, approved: bool) -> Option<(Uuid, String, String)> {
        let tool_id = self.pending_tool_id.clone()?;
        let session_id = self.session_id?;
        let write_token = self.write_token.clone()?;

        if let Some(item) = self
            .items
            .iter_mut()
            .find(|i| matches!(i, ChatItem::Tool { id, .. } if id == &tool_id))
        {
            if let ChatItem::Tool { decision, .. } = item {
                *decision = Some(if approved {
                    ToolDecision::Approved
                } else {
                    ToolDecision::Denied
                });
            }
        }
        self.pending_tool_id = None;
        Some((session_id, write_token, tool_id))
    }
}

