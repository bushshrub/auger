use std::collections::{HashMap, HashSet};
use ratatui::widgets::ListState;
use uuid::Uuid;

use crate::types::{AppEvent, ChatItem, SnapshotMessage, SseEvent, SessionInfo, Status, ToolDecision};

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    SessionList,
    Chat,
}

pub struct App {
    pub view: View,
    pub server: String,
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
    pub fn new(server: String) -> Self {
        let mut session_list_state = ListState::default();
        session_list_state.select(Some(0));
        Self {
            view: View::SessionList,
            server,
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

            AppEvent::SessionCreated { session_id, write_token, read_token, context_window } => {
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

            AppEvent::SnapshotLoaded(msgs) => {
                self.apply_snapshot(msgs);
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
                        self.items.push(ChatItem::Reasoning { text, collapsed: true });
                        self.reasoning_idx = Some(self.items.len() - 1);
                    }
                }
            }

            SseEvent::ToolCall { id, name, arguments } => {
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

            SseEvent::ToolCallAutoApproved { id, name, arguments } => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                if let Some(item) = self.items.iter_mut().find(|i| matches!(i, ChatItem::Tool { id: tid, .. } if tid == &id)) {
                    if let ChatItem::Tool { decision, .. } = item {
                        *decision = Some(ToolDecision::Auto);
                    }
                } else {
                    self.items.push(ChatItem::Tool {
                        id,
                        name,
                        args: arguments,
                        result: None,
                        decision: Some(ToolDecision::Auto),
                    });
                }
                self.status = Status::Running;
            }

            SseEvent::ToolResult { id, content } => {
                self.assistant_idx = None;
                if let Some(item) = self.items.iter_mut().find(|i| matches!(i, ChatItem::Tool { id: tid, .. } if tid == &id)) {
                    if let ChatItem::Tool { result, decision, .. } = item {
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

            SseEvent::Metrics { prompt_tokens, completion_tokens, total_tokens } => {
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
        self.session_list_state.select(Some((i + 1) % self.sessions.len()));
    }

    pub fn session_list_prev(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let i = self.session_list_state.selected().unwrap_or(0);
        self.session_list_state.select(Some(
            if i == 0 { self.sessions.len() - 1 } else { i - 1 },
        ));
    }

    pub fn selected_session(&self) -> Option<&SessionInfo> {
        self.session_list_state.selected().and_then(|i| self.sessions.get(i))
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

    /// Populate chat history from a snapshot. Mirrors the webui's snapshotToItems logic.
    pub fn apply_snapshot(&mut self, messages: Vec<SnapshotMessage>) {
        self.items.clear();
        self.assistant_idx = None;
        self.reasoning_idx = None;

        let mut tool_idx_map: HashMap<String, usize> = HashMap::new();
        let mut last_block_ids: Vec<String> = vec![];

        for msg in messages {
            match msg {
                SnapshotMessage::User { text } => {
                    last_block_ids.clear();
                    self.items.push(ChatItem::User { text });
                }
                SnapshotMessage::Assistant { reasoning, content, tool_calls } => {
                    last_block_ids.clear();
                    if let Some(r) = reasoning {
                        if !r.is_empty() {
                            self.items.push(ChatItem::Reasoning { text: r, collapsed: true });
                        }
                    }
                    if !content.is_empty() {
                        self.items.push(ChatItem::Assistant { text: content });
                    }
                    for tc in tool_calls {
                        tool_idx_map.insert(tc.id.clone(), self.items.len());
                        last_block_ids.push(tc.id.clone());
                        self.items.push(ChatItem::Tool {
                            id: tc.id,
                            name: tc.name,
                            args: tc.arguments,
                            result: None,
                            decision: None,
                        });
                    }
                }
                SnapshotMessage::Tool { tool_call_id, content } => {
                    if let Some(&idx) = tool_idx_map.get(&tool_call_id) {
                        if let Some(ChatItem::Tool { result, .. }) = self.items.get_mut(idx) {
                            *result = Some(content);
                        }
                    }
                }
            }
        }

        // Tool calls from the last assistant block that have no follow-up are still pending.
        let pending: HashSet<String> = last_block_ids.into_iter().collect();
        for (id, idx) in &tool_idx_map {
            if pending.contains(id) {
                self.pending_tool_id = Some(id.clone());
            } else if let Some(ChatItem::Tool { decision, .. }) = self.items.get_mut(*idx) {
                if decision.is_none() {
                    *decision = Some(ToolDecision::Approved);
                }
            }
        }

        self.scroll_to_bottom();
    }

    /// Returns (session_id, write_token, tool_call_id) if there's a pending tool.
    pub fn approve_tool(&mut self, approved: bool) -> Option<(Uuid, String, String)> {
        let tool_id = self.pending_tool_id.clone()?;
        let session_id = self.session_id?;
        let write_token = self.write_token.clone()?;

        if let Some(item) = self.items.iter_mut().find(|i| matches!(i, ChatItem::Tool { id, .. } if id == &tool_id)) {
            if let ChatItem::Tool { decision, .. } = item {
                *decision = Some(if approved { ToolDecision::Approved } else { ToolDecision::Denied });
            }
        }
        self.pending_tool_id = None;
        Some((session_id, write_token, tool_id))
    }
}
