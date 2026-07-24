use crate::command::Command;
use crate::completion::Completion;
use crate::types::AppEvent;
use crate::types::ChatItem;
use crate::types::SessionInfo;
use crate::types::SseEvent;
use crate::types::Status;
use crate::types::ToolDecision;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;
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

/// Where the message pane's rendered rows landed on screen, recorded each
/// frame so mouse clicks can be mapped back to the chat item that produced
/// them. Rendering is the only place that knows how items wrap into rows.
#[derive(Default)]
pub struct MsgLayout {
    /// The pane rect, borders included.
    pub area: Rect,
    /// Index of the first rendered row visible at the top of the pane.
    pub scroll_top: u16,
    /// Chat item index behind each rendered row; `None` for spacer rows.
    pub row_items: Vec<Option<usize>>,
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
    /// Tool calls awaiting a decision, oldest first. The server wants one
    /// response per call, so a batch has to be answered one at a time.
    pub pending_tools: Vec<String>,
    /// Token counters shown live in the header. `tokens_out` is reconciled
    /// with the server's usage at the end of each turn.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Characters streamed during the in-flight turn, used to estimate output
    /// tokens before usage arrives.
    live_chars: usize,
    pub status: Status,
    pub input: String,
    /// Byte offset of the edit cursor within `input`, always on a char boundary.
    pub cursor: usize,
    /// Slash-command popup state; the match list itself comes from `input`.
    pub completion: Completion,
    /// Lines above the bottom that the user has scrolled. 0 = pinned to bottom.
    pub scroll_from_bottom: u16,
    /// Rendered-row → chat-item map from the last frame, used for click hits.
    pub msg_layout: MsgLayout,

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
            pending_tools: vec![],
            tokens_in: 0,
            tokens_out: 0,
            live_chars: 0,
            status: Status::Connecting,
            input: String::new(),
            cursor: 0,
            completion: Completion::default(),
            scroll_from_bottom: 0,
            msg_layout: MsgLayout::default(),
            assistant_idx: None,
            reasoning_idx: None,
            error_msg: None,
        }
    }

    /// The tool call currently being asked about: the head of the queue.
    pub fn pending_tool(&self) -> Option<&str> {
        self.pending_tools.first().map(String::as_str)
    }

    pub fn has_pending_tool(&self) -> bool {
        !self.pending_tools.is_empty()
    }

    /// Output tokens including an estimate for the turn still streaming, so
    /// the counter moves while the model is talking.
    pub fn tokens_out_live(&self) -> u64 {
        self.tokens_out + (self.live_chars / 4) as u64
    }

    fn queue_pending_tool(&mut self, id: String) {
        if !id.is_empty() && !self.pending_tools.contains(&id) {
            self.pending_tools.push(id);
        }
    }

    fn resolve_pending_tool(&mut self, id: &str) {
        self.pending_tools.retain(|t| t != id);
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
                self.pending_tools.clear();
                self.tokens_in = 0;
                self.tokens_out = 0;
                self.live_chars = 0;
                self.status = Status::Idle;
                self.assistant_idx = None;
                self.reasoning_idx = None;
                self.view = View::Chat;
            }

            AppEvent::SnapshotLines(lines) => {
                self.apply_snapshot_lines(&lines);
                // Only go idle if there's no pending tool waiting for approval.
                if !self.has_pending_tool() {
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

    /// Index of the chat item for tool call `id`, creating it if this is the
    /// first fragment we've seen. Some providers omit the id on continuation
    /// deltas, so an empty id attaches to the tool call still in flight.
    fn tool_item(&mut self, id: &str, name: &str) -> usize {
        let existing = if id.is_empty() {
            self.items
                .iter()
                .rposition(|i| matches!(i, ChatItem::Tool { result: None, .. }))
        } else {
            self.items
                .iter()
                .position(|i| matches!(i, ChatItem::Tool { id: tid, .. } if tid == id))
        };

        match existing {
            Some(idx) => idx,
            None => {
                self.items.push(ChatItem::Tool {
                    id: id.to_string(),
                    name: name.to_string(),
                    args: String::new(),
                    result: None,
                    decision: None,
                    expanded: false,
                });
                self.items.len() - 1
            }
        }
    }

    fn handle_sse(&mut self, ev: SseEvent) {
        match ev {
            SseEvent::Content { text } => {
                self.reasoning_idx = None;
                self.live_chars += text.chars().count();
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
                self.live_chars += text.chars().count();
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

            // A fragment of a streaming tool call. Appending to the matching
            // item is what keeps one call from fragmenting into many.
            SseEvent::ToolCallDelta {
                id,
                name,
                arguments,
            } => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                let idx = self.tool_item(&id, &name);
                if let Some(ChatItem::Tool {
                    name: n, args, ..
                }) = self.items.get_mut(idx)
                {
                    // Providers may only send the name on the first delta.
                    if n.is_empty() && !name.is_empty() {
                        *n = name;
                    }
                    args.push_str(&arguments);
                }
                self.status = Status::Running;
            }

            SseEvent::ToolCallComplete {
                id,
                name,
                arguments,
            } => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                let idx = self.tool_item(&id, &name);
                if let Some(ChatItem::Tool {
                    name: n, args, ..
                }) = self.items.get_mut(idx)
                {
                    if !name.is_empty() {
                        *n = name;
                    }
                    // The complete arguments supersede the assembled deltas,
                    // which may have been truncated or malformed.
                    if !arguments.is_empty() {
                        *args = arguments;
                    }
                }
                self.queue_pending_tool(id);
                self.status = Status::Running;
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

            SseEvent::ToolResult {
                id,
                content,
                denied,
            } => {
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
                        if denied {
                            *decision = Some(ToolDecision::Denied);
                        } else if decision.is_none() {
                            *decision = Some(ToolDecision::Approved);
                        }
                    }
                }
                self.resolve_pending_tool(&id);
            }

            SseEvent::Metrics {
                prompt_tokens,
                completion_tokens,
                total_tokens,
            } => {
                self.ctx_used = total_tokens
                    .or_else(|| prompt_tokens.zip(completion_tokens).map(|(p, c)| p + c))
                    .unwrap_or(self.ctx_used);
                // Real usage supersedes the streaming estimate for this turn.
                self.tokens_in = prompt_tokens.unwrap_or(self.tokens_in);
                self.tokens_out += completion_tokens.unwrap_or((self.live_chars / 4) as u64);
                self.live_chars = 0;
            }

            SseEvent::TurnComplete => {
                self.assistant_idx = None;
                self.reasoning_idx = None;
                self.live_chars = 0;
                // A turn that ended asking for consent is not idle: the user
                // still owes the server a decision per queued call.
                if !self.has_pending_tool() {
                    self.status = Status::Idle;
                }
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
        self.pending_tools.clear();
        self.tokens_in = 0;
        self.tokens_out = 0;
        self.live_chars = 0;
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

    /// Chat item under a screen cell, using the last frame's layout.
    pub fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.msg_layout.area;
        if area.width < 2 || area.height < 2 {
            return None;
        }
        // The pane is bordered, so the inner region is inset by one cell.
        let inside = column > area.x
            && column < area.x + area.width - 1
            && row > area.y
            && row < area.y + area.height - 1;
        if !inside {
            return None;
        }

        let offset = row - area.y - 1;
        let index = self.msg_layout.scroll_top as usize + offset as usize;
        self.msg_layout.row_items.get(index).copied().flatten()
    }

    /// Expand or collapse the item at `index`. Returns whether anything
    /// changed, so callers can ignore clicks on item kinds that don't expand.
    pub fn toggle_expanded(&mut self, index: usize) -> bool {
        match self.items.get_mut(index) {
            Some(ChatItem::Reasoning { collapsed, .. }) => {
                *collapsed = !*collapsed;
                true
            }
            Some(ChatItem::Tool { expanded, .. }) => {
                *expanded = !*expanded;
                true
            }
            _ => false,
        }
    }

    /// Handle a left click in the chat view. Returns whether it hit anything.
    pub fn click(&mut self, column: u16, row: u16) -> bool {
        match self.item_at(column, row) {
            Some(index) => self.toggle_expanded(index),
            None => false,
        }
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

    // ── input editing ────────────────────────────────────────────────────
    // `cursor` is a byte offset into `input`, always on a char boundary.

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.completion.reset();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if let Some((offset, _)) = self.input[..self.cursor].char_indices().next_back() {
            self.input.remove(offset);
            self.cursor = offset;
            self.completion.reset();
        }
    }

    /// Delete the character under the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
            self.completion.reset();
        }
    }

    /// Whether the slash-command popup is showing for the current input.
    pub fn completion_open(&self) -> bool {
        self.completion.is_open(&self.input)
    }

    /// Replace the input with the highlighted command. Returns whether a
    /// completion was applied.
    pub fn accept_completion(&mut self) -> bool {
        match self.completion.accept(&self.input) {
            Some(completed) => {
                self.input = completed;
                self.cursor = self.input.len();
                true
            }
            None => false,
        }
    }

    pub fn cursor_left(&mut self) {
        if let Some((offset, _)) = self.input[..self.cursor].char_indices().next_back() {
            self.cursor = offset;
        }
    }

    pub fn cursor_right(&mut self) {
        if let Some(c) = self.input[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub fn cursor_end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Display columns between the start of the input and the cursor. Byte
    /// offsets and screen columns diverge for non-ASCII text.
    pub fn cursor_column(&self) -> usize {
        self.input[..self.cursor].width()
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.completion.reset();
    }

    /// Take the current input as a command, clearing the input if it was one.
    pub fn take_command(&mut self) -> Option<Command> {
        let command = crate::command::parse(&self.input)?;
        self.clear_input();
        Some(command)
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.items.push(ChatItem::Error { text: text.into() });
        self.scroll_to_bottom();
    }

    /// Show text in the chat pane without sending it to the server.
    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.items.push(ChatItem::Assistant { text: text.into() });
        self.assistant_idx = None;
        self.scroll_to_bottom();
    }

    pub fn send_message(&mut self) -> Option<(Uuid, String, String)> {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.status != Status::Idle {
            return None;
        }
        let session_id = self.session_id?;
        let write_token = self.write_token.clone()?;
        self.items.push(ChatItem::User { text: text.clone() });
        self.clear_input();
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
                                                expanded: false,
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

        // Calls from the last assistant block that never got a result are still
        // waiting on the user; queue them in transcript order.
        self.pending_tools.clear();
        let mut tools: Vec<(String, usize)> = tool_idx_map.into_iter().collect();
        tools.sort_by_key(|(_, idx)| *idx);
        for (id, idx) in tools {
            let unresolved = matches!(
                self.items.get(idx),
                Some(ChatItem::Tool { result: None, .. })
            );
            if last_block_ids.contains(&id) && unresolved {
                self.pending_tools.push(id);
            } else if let Some(ChatItem::Tool { decision, .. }) = self.items.get_mut(idx) {
                if decision.is_none() {
                    *decision = Some(ToolDecision::Approved);
                }
            }
        }

        self.scroll_to_bottom();
    }

    /// Decide the tool call at the head of the queue. Returns the call to
    /// answer, or `None` if nothing is pending.
    pub fn decide_tool(&mut self, approved: bool) -> Option<(Uuid, String, String)> {
        let (session_id, write_token, mut ids) = self.decide_tools(approved, 1)?;
        Some((session_id, write_token, ids.remove(0)))
    }

    /// Decide every queued tool call at once. The server wants one response
    /// per call, so this hands back the whole list to answer.
    pub fn decide_all_tools(&mut self, approved: bool) -> Option<(Uuid, String, Vec<String>)> {
        let count = self.pending_tools.len();
        self.decide_tools(approved, count)
    }

    fn decide_tools(
        &mut self,
        approved: bool,
        count: usize,
    ) -> Option<(Uuid, String, Vec<String>)> {
        if count == 0 || self.pending_tools.is_empty() {
            return None;
        }
        let session_id = self.session_id?;
        let write_token = self.write_token.clone()?;
        let ids: Vec<String> = self
            .pending_tools
            .drain(..count.min(self.pending_tools.len()))
            .collect();

        for tool_id in &ids {
            if let Some(ChatItem::Tool { decision, .. }) = self
                .items
                .iter_mut()
                .find(|i| matches!(i, ChatItem::Tool { id, .. } if id == tool_id))
            {
                *decision = Some(if approved {
                    ToolDecision::Approved
                } else {
                    ToolDecision::Denied
                });
            }
        }
        Some((session_id, write_token, ids))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Build a chat app and render it once, so `msg_layout` holds a real
    /// row → item map produced by the actual rendering path.
    fn rendered(items: Vec<ChatItem>, width: u16, height: u16) -> App {
        let mut app = App::new();
        app.view = View::Chat;
        app.status = Status::Idle;
        app.items = items;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        app
    }

    fn reasoning(text: &str) -> ChatItem {
        ChatItem::Reasoning {
            text: text.to_string(),
            collapsed: true,
        }
    }

    fn is_collapsed(app: &App, index: usize) -> bool {
        match &app.items[index] {
            ChatItem::Reasoning { collapsed, .. } => *collapsed,
            other => panic!("item {index} is not reasoning: {other:?}"),
        }
    }

    /// Screen row of the first row belonging to `index`, if it is on screen.
    fn row_of(app: &App, index: usize) -> u16 {
        let offset = app
            .msg_layout
            .row_items
            .iter()
            .position(|i| *i == Some(index))
            .expect("item was not rendered");
        let scroll_top = app.msg_layout.scroll_top as usize;
        assert!(offset >= scroll_top, "item is scrolled off the top");
        app.msg_layout.area.y + 1 + (offset - scroll_top) as u16
    }

    fn tool_of(app: &App, index: usize) -> (String, String) {
        match &app.items[index] {
            ChatItem::Tool { name, args, .. } => (name.clone(), args.clone()),
            other => panic!("item {index} is not a tool: {other:?}"),
        }
    }

    /// The bug from the screenshot: each streamed argument fragment became its
    /// own tool item, shredding `cd auger && git status` into six phantom calls.
    #[test]
    fn streaming_argument_deltas_build_one_tool_call() {
        let mut app = App::new();
        for fragment in ["{\"comm", "and\":\"cd auger ", "&& git status\"}"] {
            app.handle_app_event(AppEvent::Sse(SseEvent::ToolCallDelta {
                id: "call_1".into(),
                name: "shell".into(),
                arguments: fragment.into(),
            }));
        }

        assert_eq!(app.items.len(), 1, "deltas must not create separate items");
        let (name, args) = tool_of(&app, 0);
        assert_eq!(name, "shell");
        assert_eq!(args, r#"{"command":"cd auger && git status"}"#);
    }

    #[test]
    fn complete_arguments_supersede_the_assembled_deltas() {
        let mut app = App::new();
        app.handle_app_event(AppEvent::Sse(SseEvent::ToolCallDelta {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: "{\"comm".into(),
        }));
        app.handle_app_event(AppEvent::Sse(SseEvent::ToolCallComplete {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: r#"{"command":"ls"}"#.into(),
        }));

        assert_eq!(app.items.len(), 1);
        assert_eq!(tool_of(&app, 0).1, r#"{"command":"ls"}"#);
    }

    #[test]
    fn deltas_without_an_id_attach_to_the_call_in_flight() {
        let mut app = App::new();
        app.handle_app_event(AppEvent::Sse(SseEvent::ToolCallDelta {
            id: "call_1".into(),
            name: "shell".into(),
            arguments: "{\"a\":".into(),
        }));
        app.handle_app_event(AppEvent::Sse(SseEvent::ToolCallDelta {
            id: String::new(),
            name: String::new(),
            arguments: "1}".into(),
        }));

        assert_eq!(app.items.len(), 1);
        assert_eq!(tool_of(&app, 0), ("shell".into(), r#"{"a":1}"#.into()));
    }

    #[test]
    fn separate_tool_calls_stay_separate() {
        let mut app = App::new();
        for id in ["call_1", "call_2"] {
            app.handle_app_event(AppEvent::Sse(SseEvent::ToolCallDelta {
                id: id.into(),
                name: "shell".into(),
                arguments: "{}".into(),
            }));
        }
        assert_eq!(app.items.len(), 2);
    }

    #[test]
    fn consent_events_parse_without_a_type_tag() {
        // The server serialises ToolCallRequest directly; expecting a tagged
        // enum here produced "parse error: missing field `type`".
        let raw = r#"{"type":"tool_consent_required","tool_calls":[
            {"id":"call_1","name":"shell","arguments":"{\"command\":\"ls\"}"}]}"#;
        let parsed: crate::types::RawSessionEvent =
            serde_json::from_str(raw).expect("consent event should parse");
        let events = crate::types::transform_raw_event(parsed);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            SseEvent::ToolCallComplete { id, name, .. } if id == "call_1" && name == "shell"
        ));
    }

    /// The server sends the whole `ToolCallResult` object, so parsing `result`
    /// as a string failed with "invalid type: map, expected a string".
    #[test]
    fn tool_results_parse_as_outcome_objects() {
        let raw = r#"{"type":"tool_call_result","id":"call_1",
            "result":{"tool_call_id":"call_1",
                      "outcome":{"success":{"content":[{"text":{"text":"ok"}}]}}}}"#;
        let parsed: crate::types::RawSessionEvent =
            serde_json::from_str(raw).expect("tool result should parse");
        let events = crate::types::transform_raw_event(parsed);
        assert!(matches!(
            &events[0],
            SseEvent::ToolResult { id, content, denied }
                if id == "call_1" && content == "ok" && !denied
        ));
    }

    #[test]
    fn denied_outcomes_carry_their_reason() {
        let raw = r#"{"type":"tool_call_result","id":"c",
            "result":{"tool_call_id":"c","outcome":{"denied":{"reason":"nope"}}}}"#;
        let parsed: crate::types::RawSessionEvent = serde_json::from_str(raw).unwrap();
        let events = crate::types::transform_raw_event(parsed);
        assert!(matches!(
            &events[0],
            SseEvent::ToolResult { content, denied, .. } if content == "nope" && *denied
        ));
    }

    #[test]
    fn interrupted_outcomes_parse() {
        let raw = r#"{"type":"tool_call_result","id":"c",
            "result":{"tool_call_id":"c","outcome":"interrupted"}}"#;
        let parsed: crate::types::RawSessionEvent = serde_json::from_str(raw).unwrap();
        assert_eq!(crate::types::transform_raw_event(parsed).len(), 1);
    }

    // ── tool approval queue ──────────────────────────────────────────────

    fn with_session(app: &mut App) {
        app.session_id = Some(Uuid::new_v4());
        app.write_token = Some("t".into());
    }

    fn consent(id: &str) -> AppEvent {
        AppEvent::Sse(SseEvent::ToolCallComplete {
            id: id.into(),
            name: "shell".into(),
            arguments: "{}".into(),
        })
    }

    /// The bug: a turn asking for four calls only ever remembered the last, so
    /// the other three were never answered and the session hung.
    #[test]
    fn every_call_in_a_batch_stays_pending() {
        let mut app = App::new();
        with_session(&mut app);
        for id in ["c1", "c2", "c3", "c4"] {
            app.handle_app_event(consent(id));
        }
        assert_eq!(app.pending_tools, ["c1", "c2", "c3", "c4"]);

        let mut answered = vec![];
        while let Some((_, _, id)) = app.decide_tool(true) {
            answered.push(id);
        }
        assert_eq!(answered, ["c1", "c2", "c3", "c4"]);
        assert!(!app.has_pending_tool());
    }

    /// The batch keys have to be on screen, otherwise the only way out of a
    /// multi-call prompt is invisible.
    #[test]
    fn a_batch_prompt_offers_the_bulk_keys() {
        let mut app = App::new();
        app.view = View::Chat;
        with_session(&mut app);
        for id in ["c1", "c2"] {
            app.handle_app_event(consent(id));
        }

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, &mut app))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(screen.contains("[a] approve all 2"), "screen was:\n{screen}");
        assert!(screen.contains("[d] deny all 2"));
    }

    #[test]
    fn approving_all_answers_the_whole_batch() {
        let mut app = App::new();
        with_session(&mut app);
        for id in ["c1", "c2"] {
            app.handle_app_event(consent(id));
        }
        let (_, _, ids) = app.decide_all_tools(true).expect("a batch is pending");
        assert_eq!(ids, ["c1", "c2"]);
        assert!(!app.has_pending_tool());
    }

    #[test]
    fn duplicate_consent_events_do_not_queue_twice() {
        let mut app = App::new();
        with_session(&mut app);
        app.handle_app_event(consent("c1"));
        app.handle_app_event(consent("c1"));
        assert_eq!(app.pending_tools, ["c1"]);
    }

    #[test]
    fn a_result_clears_only_its_own_pending_call() {
        let mut app = App::new();
        with_session(&mut app);
        for id in ["c1", "c2"] {
            app.handle_app_event(consent(id));
        }
        app.handle_app_event(AppEvent::Sse(SseEvent::ToolResult {
            id: "c1".into(),
            content: "ok".into(),
            denied: false,
        }));
        assert_eq!(app.pending_tools, ["c2"]);
    }

    #[test]
    fn a_turn_ending_with_pending_calls_is_not_idle() {
        let mut app = App::new();
        with_session(&mut app);
        app.handle_app_event(consent("c1"));
        app.handle_app_event(AppEvent::Sse(SseEvent::TurnComplete));
        assert_eq!(app.status, Status::Running);
    }

    // ── token counter ────────────────────────────────────────────────────

    #[test]
    fn streamed_text_moves_the_counter_before_usage_arrives() {
        let mut app = App::new();
        app.handle_app_event(AppEvent::Sse(SseEvent::Content {
            text: "a".repeat(400),
        }));
        assert_eq!(app.tokens_out_live(), 100, "roughly 4 chars per token");
    }

    #[test]
    fn reported_usage_supersedes_the_estimate() {
        let mut app = App::new();
        app.handle_app_event(AppEvent::Sse(SseEvent::Content {
            text: "a".repeat(400),
        }));
        app.handle_app_event(AppEvent::Sse(SseEvent::Metrics {
            prompt_tokens: Some(1200),
            completion_tokens: Some(80),
            total_tokens: Some(1280),
        }));
        assert_eq!(app.tokens_in, 1200);
        assert_eq!(app.tokens_out_live(), 80);
    }

    #[test]
    fn output_tokens_accumulate_across_turns() {
        let mut app = App::new();
        for _ in 0..2 {
            app.handle_app_event(AppEvent::Sse(SseEvent::Metrics {
                prompt_tokens: Some(10),
                completion_tokens: Some(50),
                total_tokens: Some(60),
            }));
        }
        assert_eq!(app.tokens_out_live(), 100);
    }

    // ── input editing ────────────────────────────────────────────────────

    fn typed(text: &str) -> App {
        let mut app = App::new();
        for c in text.chars() {
            app.insert_char(c);
        }
        app
    }

    #[test]
    fn arrows_move_the_cursor_and_insert_in_place() {
        let mut app = typed("helo");
        app.cursor_left();
        app.insert_char('l');
        assert_eq!(app.input, "hello");
        assert_eq!(app.cursor, 4);
    }

    #[test]
    fn cursor_stops_at_both_ends() {
        let mut app = typed("ab");
        app.cursor_left();
        app.cursor_left();
        app.cursor_left();
        assert_eq!(app.cursor, 0);
        for _ in 0..5 {
            app.cursor_right();
        }
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn backspace_and_delete_act_around_the_cursor() {
        let mut app = typed("abc");
        app.cursor_left();
        app.backspace();
        assert_eq!(app.input, "ac");
        app.cursor_home();
        app.delete();
        assert_eq!(app.input, "c");
    }

    #[test]
    fn home_and_end_jump_to_the_edges() {
        let mut app = typed("hello");
        app.cursor_home();
        assert_eq!(app.cursor, 0);
        app.cursor_end();
        assert_eq!(app.cursor, 5);
    }

    #[test]
    fn editing_multibyte_text_stays_on_char_boundaries() {
        // Byte offsets and display columns both diverge from char counts here.
        let mut app = typed("héllo");
        app.cursor_home();
        app.cursor_right();
        app.cursor_right();
        assert_eq!(app.cursor, 3, "é is two bytes");
        assert_eq!(app.cursor_column(), 2, "but one column");
        app.backspace();
        assert_eq!(app.input, "hllo");
    }

    #[test]
    fn wide_characters_advance_the_cursor_two_columns() {
        let app = typed("日本");
        assert_eq!(app.cursor_column(), 4);
    }

    #[test]
    fn sending_a_message_resets_the_cursor() {
        let mut app = typed("hi");
        app.session_id = Some(Uuid::new_v4());
        app.write_token = Some("t".into());
        app.status = Status::Idle;
        assert!(app.send_message().is_some());
        assert_eq!(app.input, "");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn slash_commands_are_taken_out_of_the_input() {
        let mut app = typed("/new");
        assert_eq!(
            app.take_command(),
            Some(crate::command::Command::New { model: None })
        );
        assert_eq!(app.input, "", "the command should not linger as a message");
    }

    #[test]
    fn ordinary_messages_are_left_alone() {
        let mut app = typed("hello");
        assert_eq!(app.take_command(), None);
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn clicking_a_tool_call_toggles_its_diff() {
        let mut app = rendered(
            vec![ChatItem::Tool {
                id: "t".into(),
                name: "edit_file".into(),
                args: r#"{"path":"/a.rs","old_string":"a","new_string":"b"}"#.into(),
                result: None,
                decision: None,
                expanded: false,
            }],
            60,
            20,
        );
        let row = row_of(&app, 0);
        assert!(app.click(4, row));
        match &app.items[0] {
            ChatItem::Tool { expanded, .. } => assert!(*expanded),
            other => panic!("expected a tool: {other:?}"),
        }
    }

    #[test]
    fn clicking_reasoning_expands_it() {
        let mut app = rendered(vec![reasoning("deep thoughts")], 40, 20);
        assert!(is_collapsed(&app, 0));

        let row = row_of(&app, 0);
        assert!(app.click(4, row));
        assert!(!is_collapsed(&app, 0));
    }

    #[test]
    fn clicking_an_expanded_block_collapses_it_again() {
        let mut app = rendered(vec![reasoning("deep thoughts")], 40, 20);
        let row = row_of(&app, 0);

        assert!(app.click(4, row));
        assert!(!is_collapsed(&app, 0));
        assert!(app.click(4, row));
        assert!(is_collapsed(&app, 0));
    }

    #[test]
    fn expanding_shows_the_full_text() {
        let mut app = rendered(vec![reasoning("secret reasoning")], 40, 20);
        let row = row_of(&app, 0);
        app.click(4, row);

        let app = rendered(app.items, 40, 20);
        let rows = app.msg_layout.row_items.len();
        assert!(rows > 1, "expanded block should occupy more than one row");
    }

    #[test]
    fn clicking_a_non_reasoning_item_does_nothing() {
        let mut app = rendered(
            vec![ChatItem::User {
                text: "hello".to_string(),
            }],
            40,
            20,
        );
        let row = row_of(&app, 0);
        assert!(!app.click(4, row));
    }

    #[test]
    fn clicking_the_spacer_row_does_nothing() {
        let mut app = rendered(vec![reasoning("thoughts")], 40, 20);
        // The first rendered row is the spacer that precedes every item.
        let spacer = app.msg_layout.area.y + 1;
        let top = app.msg_layout.scroll_top as usize;
        assert_eq!(app.msg_layout.row_items[top], None);
        assert!(!app.click(4, spacer));
        assert!(is_collapsed(&app, 0));
    }

    #[test]
    fn clicking_outside_the_pane_does_nothing() {
        let mut app = rendered(vec![reasoning("thoughts")], 40, 20);
        let row = row_of(&app, 0);
        // On the border, and above the pane entirely.
        assert!(!app.click(app.msg_layout.area.x, row));
        assert!(!app.click(4, 0));
        assert!(is_collapsed(&app, 0));
    }

    #[test]
    fn clicks_hit_the_right_item_among_several() {
        let mut app = rendered(
            vec![
                ChatItem::User {
                    text: "q".to_string(),
                },
                reasoning("first"),
                ChatItem::Assistant {
                    text: "a".to_string(),
                },
                reasoning("second"),
            ],
            40,
            30,
        );

        let row = row_of(&app, 3);
        assert!(app.click(4, row));
        assert!(!is_collapsed(&app, 3), "clicked block should expand");
        assert!(is_collapsed(&app, 1), "other block should be untouched");
    }

    #[test]
    fn clicks_account_for_scrolling() {
        // More content than fits, so the pane is scrolled and screen rows no
        // longer line up with row indices.
        let items = vec![
            ChatItem::Assistant {
                text: "filler\n\nfiller\n\nfiller\n\nfiller".to_string(),
            },
            reasoning("target"),
        ];
        let mut app = rendered(items, 40, 10);

        let row = row_of(&app, 1);
        assert!(app.msg_layout.scroll_top > 0, "expected the pane to scroll");
        assert!(app.click(4, row));
        assert!(!is_collapsed(&app, 1));
    }

    #[test]
    fn stale_layout_from_an_empty_pane_is_not_clickable() {
        let mut app = rendered(vec![], 40, 20);
        assert!(!app.click(4, 5));
    }
}
