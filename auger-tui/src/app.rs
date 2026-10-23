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
use uuid::Uuid;

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
    /// Rows of the streaming item at `flushed` already printed to scrollback:
    /// (item index, rows).
    pub committed: Option<(usize, usize)>,
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
    /// First preview row shown in the approval panel.
    pub approval_scroll: usize,
    /// The transcript pager is open.
    pub pager_open: bool,
    /// Pager rows scrolled up from the bottom.
    pub pager_from_bottom: usize,
    /// Largest useful `pager_from_bottom`, set when the pager draws.
    pub pager_max: std::cell::Cell<usize>,
    /// Text typed into the focused "No, and tell auger" row.
    pub approval_note: String,
    /// A denial note to send as the next message once the batch is withheld.
    queued_note: Option<String>,
    /// Send the composer's text on the next loop turn.
    pub submit_requested: bool,
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
    pub(crate) live_chars: usize,
    pub status: Status,
    pub input: String,
    /// Byte offset of the edit cursor within `input`, always on a char boundary.
    pub cursor: usize,
    /// Slash-command popup state; the match list itself comes from `input`.
    pub completion: Completion,

    // In-progress streaming indices
    pub assistant_idx: Option<usize>,
    pub reasoning_idx: Option<usize>,

    /// Folds the session's records into `items`.
    pub(crate) fold: crate::fold::Fold,

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
            committed: None,
            pending_tools: vec![],
            approval_selected: 0,
            approval_scroll: 0,
            pager_open: false,
            pager_from_bottom: 0,
            pager_max: std::cell::Cell::new(0),
            approval_note: String::new(),
            queued_note: None,
            submit_requested: false,
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
            fold: crate::fold::Fold::default(),
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

    pub(crate) fn resolve_pending_tool(&mut self, id: &str) {
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
                self.fold.open_tail(self.items.len());
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
                self.fold.open_tail(self.items.len());
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
                self.fold.open_tail(self.items.len());
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
                self.fold.open_tail(self.items.len());
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
                self.status = Status::Running;
            }

            SseEvent::ToolConsent {
                id,
                name,
                arguments,
            } => {
                self.handle_sse(SseEvent::ToolCallComplete {
                    id: id.clone(),
                    name,
                    arguments,
                });
                self.queue_pending_tool(id);
            }

            SseEvent::Record(record) => self.apply_record(&record),

            SseEvent::ToolResultsWithheld => {
                self.status = Status::Idle;
                if let Some(note) = self.queued_note.take() {
                    self.input = note;
                    self.submit_requested = true;
                }
            }

            SseEvent::StreamClosed => {
                self.status = Status::Idle;
            }

            SseEvent::Error { message } => self.push_error(message),
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
        self.fold = crate::fold::Fold::default();
        self.flushed = 0;
        self.committed = None;
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
            // A finished exploratory call waits while the next call may
            // still fold into the same "Explored" cell.
            ChatItem::Tool { result, .. } => {
                result.is_some()
                    && !crate::explore::holds(&self.items, index, self.status == Status::Idle)
            }
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
        // The message shows when its turns record arrives.
        self.clear_input();
        self.status = Status::Running;
        self.assistant_idx = None;
        self.reasoning_idx = None;
        Some((session_id, write_token, text))
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

    /// Scroll the approval preview by `delta` rows. Clamped when drawn.
    pub fn approval_scroll_by(&mut self, delta: isize) {
        self.approval_scroll = self.approval_scroll.saturating_add_signed(delta);
    }

    /// Scroll the pager by `delta` rows; positive scrolls up.
    pub fn pager_scroll_by(&mut self, delta: isize) {
        let max = self.pager_max.get();
        self.pager_from_bottom = self.pager_from_bottom.saturating_add_signed(delta).min(max);
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
        self.approval_scroll = 0;
        let note = std::mem::take(&mut self.approval_note);
        if !choice.approved() && !note.trim().is_empty() {
            self.queued_note = Some(note.trim().to_string());
        }
        self.decide_tool(choice.approved())
            .map(|(sid, token, id)| (sid, token, vec![id]))
    }

    pub fn decide_tool(&mut self, approved: bool) -> Option<(Uuid, String, String)> {
        let (session_id, write_token, mut ids) = self.decide_tools(approved, 1)?;
        Some((session_id, write_token, ids.remove(0)))
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
    fn snapshot_records_restore_block_events() {
        let mut app = App::new();
        let lines = vec![
            r#"{"seq":0,"ts":"2026-10-23T00:00:00Z","type":"session","data":{"version":2}}"#.to_string(),
            r#"{"seq":1,"ts":"2026-10-23T00:00:00Z","type":"turns","data":{"push_turn":{"input":{"entries":[{"user":{"message":"hello"}}]}}}}"#.to_string(),
            r#"{"seq":2,"ts":"2026-10-23T00:00:01Z","type":"turns","data":{"push_turn":{"output":[{"reasoning":{"text":"think"}},{"text":"answer"},{"tool_call":{"id":"call_1","name":"shell","arguments":"{\"command\":\"pwd\"}"}}]}}}"#.to_string(),
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
            SseEvent::ToolConsent { id, name, .. } if id == "call_1" && name == "shell"
        ));
    }

    #[test]
    fn record_events_parse_and_carry_the_record() {
        let raw = r#"{"type":"record","record":{"seq":4,"ts":"2026-10-23T00:00:00Z",
            "type":"stream","data":"started"}}"#;
        let parsed: crate::types::RawSessionEvent = serde_json::from_str(raw).unwrap();
        let events = crate::types::transform_raw_event(parsed);
        assert!(matches!(
            &events[0],
            SseEvent::Record(record) if record["seq"] == 4
        ));
    }

    // -- records -----------------------------------------------------------

    fn record(seq: u64, kind: &str, data: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "seq": seq, "ts": "2026-10-23T00:00:00Z", "type": kind, "data": data })
    }

    // ── tool approval queue ──────────────────────────────────────────────

    fn with_session(app: &mut App) {
        app.session_id = Some(Uuid::new_v4());
        app.write_token = Some("t".into());
    }

    fn consent(id: &str) -> AppEvent {
        AppEvent::Sse(SseEvent::ToolConsent {
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
        app.apply_record(&record(
            0,
            "tool",
            serde_json::json!({ "finished": { "tool_call_id": "c1", "outcome": "interrupted" } }),
        ));
        assert_eq!(app.pending_tools, ["c2"]);
    }

    #[test]
    fn a_turn_ending_with_pending_calls_is_not_idle() {
        let mut app = App::new();
        with_session(&mut app);
        app.handle_app_event(consent("c1"));
        app.apply_record(&record(
            0,
            "stream",
            serde_json::json!({ "completed": { "usage": null, "stop_reason": "tool_calls" } }),
        ));
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
        app.apply_record(&record(
            0,
            "stream",
            serde_json::json!({ "completed": { "usage": {
                "prompt_tokens": 1200, "cached_tokens": null,
                "completion_tokens": 80, "total_tokens": 1280
            }, "stop_reason": "stop" } }),
        ));
        assert_eq!(app.tokens_in, 1200);
        assert_eq!(app.tokens_out_live(), 80);
    }

    #[test]
    fn output_tokens_accumulate_across_turns() {
        let mut app = App::new();
        for seq in 0..2 {
            app.apply_record(&record(
                seq,
                "stream",
                serde_json::json!({ "completed": { "usage": {
                    "prompt_tokens": 10, "cached_tokens": null,
                    "completion_tokens": 50, "total_tokens": 60
                }, "stop_reason": "stop" } }),
            ));
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

