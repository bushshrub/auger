use crate::approval::Choice;
use crate::command::Command;
use crate::config::StatusItem;
use crate::completion::Completion;
use crate::types::AppEvent;
use crate::types::ChatItem;
use crate::types::SessionInfo;
use crate::types::SseEvent;
use crate::types::Status;
use crate::types::ToolDecision;
use unicode_width::UnicodeWidthStr;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

fn trace_tool_result_text(result: &Value) -> String {
    let Some(outcome) = result.get("outcome") else {
        return String::new();
    };
    if outcome.as_str() == Some("interrupted") {
        return "Tool call was interrupted.".to_string();
    }
    if let Some(reason) = outcome
        .get("denied")
        .and_then(|denied| denied.get("reason"))
        .and_then(|reason| reason.as_str())
    {
        return reason.to_string();
    }
    let data = outcome
        .get("success")
        .and_then(|success| success.get("content"))
        .or_else(|| outcome.get("error").and_then(|error| error.get("error")));
    data.and_then(|data| data.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("text")
                .and_then(|text| text.get("text").or(Some(text)))
                .and_then(|text| text.as_str())
        })
        .collect()
}

pub struct App {
    pub should_quit: bool,

    /// Sessions offered by `/resume`.
    pub sessions: Vec<SessionInfo>,
    /// Highlighted row of the `/resume` picker; `None` while it is closed.
    pub picker: Option<usize>,

    // Chat
    pub session_id: Option<Uuid>,
    pub model: String,
    pub write_token: Option<String>,
    pub read_token: Option<String>,
    pub ctx_window: u64,
    pub ctx_used: u64,
    pub items: Vec<ChatItem>,
    /// Items already printed into the terminal's scrollback. Everything from
    /// here on is drawn in the live area.
    pub flushed: usize,
    /// Tool calls awaiting a decision, oldest first. The server wants one
    /// response per call, so a batch has to be answered one at a time.
    pub pending_tools: Vec<String>,
    /// When the running turn started, for the working indicator. `None`
    /// while idle or waiting on an approval.
    pub working_since: Option<std::time::Instant>,
    /// A `!command` waiting for the main loop to run it on the terminal.
    pub shell_request: Option<String>,
    /// Highlighted row of the approval panel.
    pub approval_selected: usize,
    /// Fields shown in the bottom statusline, in order.
    pub statusline: Vec<StatusItem>,
    /// Token counters shown live in the header. `tokens_out` is reconciled
    /// with the server's usage at the end of each turn.
    pub tokens_in: u64,
    /// Input tokens served from the provider's prompt cache.
    pub tokens_cached: u64,
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

    // In-progress streaming indices
    pub assistant_idx: Option<usize>,
    pub reasoning_idx: Option<usize>,

    pub error_msg: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            sessions: vec![],
            picker: None,
            session_id: None,
            model: String::new(),
            write_token: None,
            read_token: None,
            ctx_window: 0,
            ctx_used: 0,
            items: vec![],
            flushed: 0,
            pending_tools: vec![],
            approval_selected: 0,
            shell_request: None,
            working_since: None,
            statusline: crate::config::TuiConfig::default().statusline,
            tokens_in: 0,
            tokens_cached: 0,
            tokens_out: 0,
            live_chars: 0,
            status: Status::Connecting,
            input: String::new(),
            cursor: 0,
            completion: Completion::default(),
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
                    self.push_notice("No sessions to resume.");
                } else {
                    self.picker = Some(0);
                }
            }

            AppEvent::SessionCreated {
                session_id,
                model,
                write_token,
                read_token,
                context_window,
            } => {
                self.session_id = Some(session_id);
                self.model = model;
                self.write_token = Some(write_token);
                self.read_token = Some(read_token);
                self.ctx_window = context_window;
                self.ctx_used = 0;
                self.reset_chat();
                self.status = Status::Idle;
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
                            started: Some(std::time::Instant::now()),
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
                self.live_chars += arguments.chars().count();
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

            SseEvent::ToolResultsWithheld => {
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
                cached_tokens,
                completion_tokens,
                total_tokens,
            } => {
                self.ctx_used = total_tokens
                    .or_else(|| prompt_tokens.zip(completion_tokens).map(|(p, c)| p + c))
                    .unwrap_or(self.ctx_used);
                // Real usage supersedes the streaming estimate for this turn.
                self.tokens_in = prompt_tokens.unwrap_or(self.tokens_in);
                self.tokens_cached = cached_tokens.unwrap_or(self.tokens_cached);
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
    }

    pub fn open_session(&mut self, info: &SessionInfo) {
        self.session_id = Some(info.session_id);
        self.model = info.model.clone();
        self.write_token = Some(info.write_token.clone());
        self.read_token = Some(info.read_token.clone());
        self.ctx_window = info.context_window;
        self.reset_chat();
        self.status = Status::Connecting;
    }

    /// Forget the current conversation. Items already in scrollback stay
    /// there; the next session's items print below them.
    fn reset_chat(&mut self) {
        self.ctx_used = 0;
        self.items.clear();
        self.flushed = 0;
        self.pending_tools.clear();
        self.tokens_in = 0;
        self.tokens_cached = 0;
        self.tokens_out = 0;
        self.live_chars = 0;
        self.assistant_idx = None;
        self.reasoning_idx = None;
    }

    /// Whether item `index` will not change again and can go to scrollback.
    /// A tool call waits for its result; streamed text waits until something
    /// follows it or the turn ends.
    fn is_finished(&self, index: usize) -> bool {
        match &self.items[index] {
            ChatItem::Tool { result, .. } => result.is_some(),
            ChatItem::User { .. } | ChatItem::Error { .. } => true,
            ChatItem::Assistant { .. } | ChatItem::Reasoning { .. } => {
                index + 1 < self.items.len() || self.status == Status::Idle
            }
        }
    }

    /// Items that just finished, in order. Stops at the first unfinished item
    /// so scrollback keeps the conversation's order.
    pub fn take_finished(&mut self) -> &[ChatItem] {
        let start = self.flushed;
        let mut end = start;
        while end < self.items.len() && self.is_finished(end) {
            end += 1;
        }
        self.flushed = end;
        &self.items[start..end]
    }

    /// Items not yet in scrollback.
    pub fn live_items(&self) -> &[ChatItem] {
        &self.items[self.flushed..]
    }

    pub fn picker_next(&mut self) {
        if let Some(i) = self.picker {
            self.picker = Some((i + 1) % self.sessions.len().max(1));
        }
    }

    pub fn picker_prev(&mut self) {
        if let Some(i) = self.picker {
            let len = self.sessions.len().max(1);
            self.picker = Some((i + len - 1) % len);
        }
    }

    /// Close the picker, returning the highlighted session if any.
    pub fn take_picked(&mut self) -> Option<SessionInfo> {
        let index = self.picker.take()?;
        self.sessions.get(index).cloned()
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

    /// Whether the input is a `!command` for the shell.
    pub fn shell_mode(&self) -> bool {
        self.input.starts_with('!')
    }

    /// Take a `!command` out of the input for the main loop to run.
    pub fn take_shell(&mut self) -> bool {
        let command = match self.input.strip_prefix('!') {
            Some(command) if !command.trim().is_empty() => command.trim().to_string(),
            _ => return false,
        };
        self.clear_input();
        self.shell_request = Some(command);
        true
    }

    /// Take the current input as a command, clearing the input if it was one.
    pub fn take_command(&mut self) -> Option<Command> {
        let command = crate::command::parse(&self.input)?;
        self.clear_input();
        Some(command)
    }

    pub fn push_error(&mut self, text: impl Into<String>) {
        self.items.push(ChatItem::Error { text: text.into() });
    }

    /// Show text in the chat pane without sending it to the server.
    pub fn push_notice(&mut self, text: impl Into<String>) {
        self.items.push(ChatItem::Assistant { text: text.into() });
        self.assistant_idx = None;
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
        Some((session_id, write_token, text))
    }

    /// Reconstruct chat items from NDJSON snapshot lines produced by the
    /// server's TraceWriter.  The trace is flat JSONL — each line has a
    /// `"kind"` discriminator (`"session"`, `"turn"`, `"event"`) with the
    /// actual payload nested under `"turn"` or `"event"`.
    pub fn apply_snapshot_lines(&mut self, lines: &[String]) {
        self.items.clear();
        self.flushed = 0;
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
                    if let Some(input_msg) = turn.get("input").and_then(|m| m.as_object()) {
                        last_block_ids.clear();
                        if let Some(entries) = input_msg.get("entries").and_then(|c| c.as_array()) {
                            for entry in entries {
                                if let Some(text) = entry
                                    .get("user")
                                    .and_then(|user| user.get("message"))
                                    .and_then(|text| text.as_str())
                                {
                                    self.items.push(ChatItem::User { text: text.to_string() });
                                } else if let Some(result) = entry.get("tool_result") {
                                    if let Some(tool_call_id) =
                                        result.get("tool_call_id").and_then(|id| id.as_str())
                                    {
                                        let text = trace_tool_result_text(result);
                                        if let Some(ChatItem::Tool { result, .. }) =
                                            self.items.iter_mut().find(|item| {
                                                matches!(item, ChatItem::Tool { id, .. } if id == tool_call_id)
                                            })
                                        {
                                            *result = Some(text);
                                        }
                                    }
                                }
                            }
                        }
                        continue;
                    }

                    if let Some(assist_msg) = turn.get("assistant").and_then(|m| m.as_object()) {
                        last_block_ids.clear();
                        if let Some(outcome) = assist_msg.get("outcome").and_then(|o| o.as_object()) {
                            let response = outcome
                                .get("completed")
                                .and_then(|payload| payload.get("response"))
                                .or_else(|| {
                                    outcome
                                        .get("incomplete")
                                        .and_then(|payload| payload.get("partial_response"))
                                });
                            if let Some(blocks) = response
                                .and_then(|response| response.get("blocks"))
                                .and_then(|blocks| blocks.as_array())
                            {
                                for block in blocks {
                                    if let Some(text) = block.get("reasoning")
                                        .or_else(|| block.get("Reasoning"))
                                        .and_then(|reasoning| reasoning.get("text"))
                                        .and_then(|text| text.as_str())
                                        .filter(|text| !text.is_empty())
                                    {
                                        self.items.push(ChatItem::Reasoning {
                                            text: text.to_string(),
                                            started: None,
                                        });
                                    } else if let Some(text) = block.get("text")
                                        .or_else(|| block.get("Text"))
                                        .and_then(|text| text.as_str())
                                        .filter(|text| !text.is_empty())
                                    {
                                        self.items.push(ChatItem::Assistant {
                                            text: text.to_string(),
                                        });
                                    } else if let Some(tc) =
                                        block.get("tool_call").or_else(|| block.get("ToolCall"))
                                    {
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

    }

    /// Decide the tool call at the head of the queue. Returns the call to
    /// answer, or `None` if nothing is pending.
    /// Name and arguments of the tool call the approval panel asks about.
    pub fn pending_tool_call(&self) -> Option<(&str, &str)> {
        let id = self.pending_tool()?;
        self.items.iter().find_map(|item| match item {
            ChatItem::Tool { id: tid, name, args, .. } if tid == id => {
                Some((name.as_str(), args.as_str()))
            }
            _ => None,
        })
    }

    /// Start or stop the working clock to match the current status.
    pub fn track_working(&mut self) {
        let working = self.status == Status::Running && !self.has_pending_tool();
        match (working, self.working_since) {
            (true, None) => self.working_since = Some(std::time::Instant::now()),
            (false, Some(_)) => self.working_since = None,
            _ => {}
        }
    }

    pub fn approval_next(&mut self) {
        let count = crate::approval::choices(self.pending_tools.len()).len();
        if count > 0 {
            self.approval_selected = (self.approval_selected + 1) % count;
        }
    }

    pub fn approval_prev(&mut self) {
        let count = crate::approval::choices(self.pending_tools.len()).len();
        if count > 0 {
            self.approval_selected = (self.approval_selected + count - 1) % count;
        }
    }

    /// The highlighted approval choice.
    pub fn approval_choice(&self) -> Option<Choice> {
        let choices = crate::approval::choices(self.pending_tools.len());
        choices.get(self.approval_selected).copied()
    }

    /// Apply `choice` to the queue. Returns the calls to answer.
    pub fn decide(&mut self, choice: Choice) -> Option<(Uuid, String, Vec<String>)> {
        if !crate::approval::choices(self.pending_tools.len()).contains(&choice) {
            return None;
        }
        self.approval_selected = 0;
        match choice.all() {
            true => self.decide_all_tools(choice.approved()),
            false => self
                .decide_tool(choice.approved())
                .map(|(sid, token, id)| (sid, token, vec![id])),
        }
    }

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

    fn reasoning(text: &str) -> ChatItem {
        ChatItem::Reasoning {
            text: text.to_string(),
            started: None,
        }
    }

    fn tool_of(app: &App, index: usize) -> (String, String) {
        match &app.items[index] {
            ChatItem::Tool { name, args, .. } => (name.clone(), args.clone()),
            other => panic!("item {index} is not a tool: {other:?}"),
        }
    }

    #[test]
    fn current_snapshot_shape_restores_block_events() {
        let mut app = App::new();
        let lines = vec![
            r#"{"kind":"turn","turn_id":"019fab1d-0000-7000-8000-000000000001","timestamp":"2026-07-28T00:00:00Z","parent_id":null,"turn":{"input":{"entries":[{"user":{"message":"hello"}}]}}}"#.to_string(),
            r#"{"kind":"turn","turn_id":"019fab1d-0000-7000-8000-000000000002","timestamp":"2026-07-28T00:00:01Z","parent_id":"019fab1d-0000-7000-8000-000000000001","turn":{"assistant":{"outcome":{"completed":{"response":{"blocks":[{"reasoning":{"text":"think"}},{"text":"answer"},{"tool_call":{"id":"call_1","name":"shell","arguments":"{\"command\":\"pwd\"}"}}]}}}}}}"#.to_string(),
        ];

        app.apply_snapshot_lines(&lines);

        assert!(matches!(&app.items[0], ChatItem::User { text } if text == "hello"));
        assert!(matches!(&app.items[1], ChatItem::Reasoning { text, .. } if text == "think"));
        assert!(matches!(&app.items[2], ChatItem::Assistant { text } if text == "answer"));
        assert_eq!(tool_of(&app, 3), ("shell".into(), r#"{"command":"pwd"}"#.into()));
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
        assert!(screen.contains("approve all 2 pending calls (a)"), "screen was:\n{screen}");
        assert!(screen.contains("No to all 2 pending calls (d)"));
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
            cached_tokens: None,
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
                cached_tokens: None,
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
    fn items_flush_in_order_once_finished() {
        let mut app = App::new();
        app.status = Status::Running;
        app.items = vec![
            ChatItem::User { text: "hi".into() },
            reasoning("hmm"),
            ChatItem::Assistant { text: "streaming".into() },
        ];
        assert_eq!(app.take_finished().len(), 2, "user and reasoning are done");
        assert_eq!(app.live_items().len(), 1, "the streaming answer stays live");

        app.status = Status::Idle;
        assert_eq!(app.take_finished().len(), 1);
        assert!(app.live_items().is_empty());
    }

    #[test]
    fn a_tool_call_holds_back_later_items_until_it_has_a_result() {
        let mut app = App::new();
        app.status = Status::Idle;
        app.items = vec![
            ChatItem::Tool {
                id: "t".into(),
                name: "shell".into(),
                args: "{}".into(),
                result: None,
                decision: None,
            },
            ChatItem::Error { text: "x".into() },
        ];
        assert!(app.take_finished().is_empty());
        if let ChatItem::Tool { result, .. } = &mut app.items[0] {
            *result = Some("ok".into());
        }
        assert_eq!(app.take_finished().len(), 2);
    }
}
