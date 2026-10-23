//! Applies session records to the app. The fold builds the transcript; its
//! effects drive status, token counters and the approval queue.

use crate::app::App;
use crate::fold::Applied;
use crate::fold::Effect;
use crate::fold::Fold;
use crate::fold::Usage;
use crate::types::ChatItem;
use crate::types::Status;
use serde_json::Value;

impl App {
    /// Rebuild the transcript from the snapshot's NDJSON records.
    pub fn apply_snapshot_lines(&mut self, lines: &[String]) {
        self.items.clear();
        self.fold = Fold::default();
        self.flushed = 0;
        self.committed = None;
        self.assistant_idx = None;
        self.reasoning_idx = None;
        for line in lines {
            if let Ok(record) = serde_json::from_str::<Value>(line) {
                self.fold.apply(&mut self.items, &record);
            }
        }
        self.pending_tools = self.fold.pending();
    }

    /// Fold one live record and act on what it means for the session.
    pub fn apply_record(&mut self, record: &Value) {
        let effect = match self.fold.apply(&mut self.items, record) {
            Applied::Ignored => return,
            Applied::Folded(effect) => effect,
        };
        // The fold may have replaced streamed items, so their indices are stale.
        self.assistant_idx = None;
        self.reasoning_idx = None;
        self.flushed = self.flushed.min(self.items.len());
        if self.committed.is_some_and(|(index, _)| index >= self.items.len()) {
            self.committed = None;
        }
        match effect {
            Some(Effect::Completed(usage)) => self.finish_turn(usage),
            Some(Effect::Stopped) => self.status = Status::Idle,
            Some(Effect::Settled(id)) => self.resolve_pending_tool(&id),
            None => {}
        }
    }

    /// The response completed. Not idle while calls await a decision, nor
    /// while the session runs the calls and streams again.
    fn finish_turn(&mut self, usage: Usage) {
        self.ctx_used = usage
            .total_tokens
            .or_else(|| usage.prompt_tokens.zip(usage.completion_tokens).map(|(p, c)| p + c))
            .unwrap_or(self.ctx_used);
        // Real usage supersedes the streaming estimate for this turn.
        self.tokens_in = usage.prompt_tokens.unwrap_or(self.tokens_in);
        self.tokens_cached = usage.cached_tokens.unwrap_or(self.tokens_cached);
        self.tokens_out += usage.completion_tokens.unwrap_or((self.live_chars / 4) as u64);
        self.live_chars = 0;

        let calls_tools = self
            .items
            .iter()
            .rev()
            .find(|i| !matches!(i, ChatItem::Reasoning { .. }))
            .is_some_and(|i| matches!(i, ChatItem::Tool { .. }));
        if !self.has_pending_tool() && !calls_tools {
            self.status = Status::Idle;
        }
    }
}
