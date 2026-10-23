//! Folds session records into the transcript. A snapshot is the records so
//! far and live `record` events come after it; both take the same path, so
//! the transcript does not depend on how it was loaded.
//!
//! Streamed deltas are provisional: they sit after the settled items until
//! the next record, which replaces them with the recorded version.

use crate::types::ChatItem;
use crate::types::ToolDecision;
use serde_json::Value;
use std::collections::HashSet;

/// What a record means for the rest of the client.
pub(crate) enum Effect {
    /// The model's response is complete; tool calls may follow.
    Completed(Usage),
    /// The stream was interrupted or failed; the session waits for input.
    Stopped,
    /// A tool call got its decision or its outcome.
    Settled(String),
}

#[derive(Default, Clone, Copy)]
pub(crate) struct Usage {
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) cached_tokens: Option<u64>,
    pub(crate) completion_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
}

pub(crate) enum Applied {
    /// Not a record, or already folded (snapshot and live stream overlap).
    Ignored,
    Folded(Option<Effect>),
}

#[derive(Default)]
pub(crate) struct Fold {
    last_seq: Option<u64>,
    /// Items each driver turn added, in order. `truncate` drops the later ones.
    turn_items: Vec<usize>,
    /// Where the partial response of an interrupted or failed stream starts.
    partial: Option<usize>,
    /// Where streamed items start; the next record replaces them.
    tail: Option<usize>,
    /// Tool calls of the last output turn.
    last_calls: Vec<String>,
    /// Tool calls that have a decision or an outcome.
    settled: HashSet<String>,
}

impl Fold {
    /// Mark `len` as the start of live items, unless already marked.
    pub(crate) fn open_tail(&mut self, len: usize) {
        self.tail.get_or_insert(len);
    }

    /// Tool calls of the last output turn that still wait for a decision.
    pub(crate) fn pending(&self) -> Vec<String> {
        self.last_calls
            .iter()
            .filter(|id| !self.settled.contains(*id))
            .cloned()
            .collect()
    }

    pub(crate) fn apply(&mut self, items: &mut Vec<ChatItem>, record: &Value) -> Applied {
        let Some(seq) = record.get("seq").and_then(Value::as_u64) else {
            return Applied::Ignored;
        };
        if self.last_seq.is_some_and(|last| seq <= last) {
            return Applied::Ignored;
        }
        self.last_seq = Some(seq);
        if let Some(tail) = self.tail.take() {
            items.truncate(tail);
        }
        let data = record.get("data").unwrap_or(&Value::Null);
        let effect = match record.get("type").and_then(Value::as_str) {
            Some("turns") => {
                self.drop_partial(items);
                self.turns(items, data);
                None
            }
            Some("stream") => self.stream(items, data),
            Some("tool") => self.tool(items, data),
            _ => None,
        };
        Applied::Folded(effect)
    }

    fn drop_partial(&mut self, items: &mut Vec<ChatItem>) {
        if let Some(start) = self.partial.take() {
            items.truncate(start);
        }
    }

    fn turns(&mut self, items: &mut Vec<ChatItem>, data: &Value) {
        if let Some(turn) = data.get("push_turn") {
            let start = items.len();
            if let Some(entries) = turn.pointer("/input/entries").and_then(Value::as_array) {
                self.last_calls.clear();
                for entry in entries {
                    push_entry(items, entry);
                }
            } else if let Some(blocks) = turn.get("output").and_then(Value::as_array) {
                self.last_calls = push_blocks(items, blocks);
            }
            self.turn_items.push(items.len() - start);
        } else if let Some(entry) = data.get("push_entry") {
            let added = push_entry(items, entry);
            if let Some(count) = self.turn_items.last_mut() {
                *count += added;
            }
        } else if let Some(len) = data.pointer("/truncate/len").and_then(Value::as_u64) {
            let len = (len as usize).min(self.turn_items.len());
            items.truncate(self.turn_items[..len].iter().sum());
            self.turn_items.truncate(len);
            self.last_calls.clear();
        }
    }

    fn stream(&mut self, items: &mut Vec<ChatItem>, data: &Value) -> Option<Effect> {
        if data.as_str() == Some("started") {
            self.drop_partial(items);
            return None;
        }
        if let Some(completed) = data.get("completed") {
            return Some(Effect::Completed(usage(completed.get("usage"))));
        }
        let (partial, error) = if let Some(interrupted) = data.get("interrupted") {
            (interrupted.get("partial"), None)
        } else if let Some(failed) = data.get("failed") {
            (
                failed.get("partial"),
                failed.pointer("/error/message").and_then(Value::as_str),
            )
        } else {
            return None;
        };
        self.drop_partial(items);
        let start = items.len();
        if let Some(blocks) = partial.and_then(Value::as_array) {
            push_blocks(items, blocks);
        }
        items.push(ChatItem::Error {
            text: error.unwrap_or("Interrupted").to_string(),
        });
        self.partial = Some(start);
        Some(Effect::Stopped)
    }

    fn tool(&mut self, items: &mut [ChatItem], data: &Value) -> Option<Effect> {
        if let Some(decided) = data.get("decided") {
            let id = decided.get("tool_call_id")?.as_str()?;
            let decision = decided.get("decision")?;
            let decision = match decision.get("allowed") {
                Some(allowed) => match allowed.pointer("/by/policy").and_then(Value::as_str) {
                    Some(node) => ToolDecision::Auto(node.to_string()),
                    None => ToolDecision::Approved,
                },
                None => ToolDecision::Denied,
            };
            if let Some(ChatItem::Tool { decision: slot, .. }) = tool_item(items, id) {
                *slot = Some(decision);
            }
            self.settled.insert(id.to_string());
            return Some(Effect::Settled(id.to_string()));
        }
        if let Some(finished) = data.get("finished") {
            let id = finished.get("tool_call_id")?.as_str()?;
            let outcome = finished.get("outcome");
            let denied = outcome.is_some_and(|o| o.get("denied").is_some());
            if let Some(ChatItem::Tool {
                result, decision, ..
            }) = tool_item(items, id)
            {
                *result = Some(outcome_text(outcome));
                if denied {
                    *decision = Some(ToolDecision::Denied);
                } else if decision.is_none() {
                    *decision = Some(ToolDecision::Approved);
                }
            }
            self.settled.insert(id.to_string());
            return Some(Effect::Settled(id.to_string()));
        }
        None
    }
}

fn tool_item<'a>(items: &'a mut [ChatItem], id: &str) -> Option<&'a mut ChatItem> {
    items
        .iter_mut()
        .find(|item| matches!(item, ChatItem::Tool { id: tid, .. } if tid == id))
}

/// Push a user entry as a user item. Returns how many items it added.
/// Harness entries (tool results, messages) show through their tool calls.
fn push_entry(items: &mut Vec<ChatItem>, entry: &Value) -> usize {
    match entry.pointer("/user/message").and_then(Value::as_str) {
        Some(text) if !text.trim().is_empty() => {
            items.push(ChatItem::User {
                text: text.to_string(),
            });
            1
        }
        _ => 0,
    }
}

/// Push the blocks of a response. Returns the ids of its tool calls.
fn push_blocks(items: &mut Vec<ChatItem>, blocks: &[Value]) -> Vec<String> {
    let mut calls = Vec::new();
    for block in blocks {
        if let Some(text) = block
            .pointer("/reasoning/text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            items.push(ChatItem::Reasoning {
                text: text.to_string(),
                started: None,
            });
        } else if let Some(text) = block
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            items.push(ChatItem::Assistant {
                text: text.to_string(),
            });
        } else if let Some(call) = block.get("tool_call") {
            let (Some(id), Some(name)) = (
                call.get("id").and_then(Value::as_str),
                call.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            let args = match call.get("arguments") {
                Some(Value::String(args)) => args.clone(),
                Some(args) => args.to_string(),
                None => String::new(),
            };
            calls.push(id.to_string());
            items.push(ChatItem::Tool {
                id: id.to_string(),
                name: name.to_string(),
                args,
                result: None,
                decision: None,
            });
        }
    }
    calls
}

fn usage(value: Option<&Value>) -> Usage {
    let field = |name: &str| value.and_then(|u| u.get(name)).and_then(Value::as_u64);
    Usage {
        prompt_tokens: field("prompt_tokens"),
        cached_tokens: field("cached_tokens"),
        completion_tokens: field("completion_tokens"),
        total_tokens: field("total_tokens"),
    }
}

/// The text of a tool call's outcome.
fn outcome_text(outcome: Option<&Value>) -> String {
    let Some(outcome) = outcome else {
        return String::new();
    };
    if outcome.as_str() == Some("interrupted") {
        return "Tool call was interrupted.".to_string();
    }
    if let Some(denied) = outcome.get("denied") {
        return denied
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("Tool call was denied.")
            .to_string();
    }
    let data = outcome
        .pointer("/success/content")
        .or_else(|| outcome.pointer("/error/error"));
    data.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.pointer("/text/text").and_then(Value::as_str))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(seq: u64, kind: &str, data: Value) -> Value {
        json!({ "seq": seq, "ts": "2026-10-23T00:00:00Z", "type": kind, "data": data })
    }

    /// A user prompt, then a response that calls one tool.
    fn history() -> Vec<Value> {
        vec![
            record(0, "turns", json!({ "push_turn": { "input": { "entries": [{ "user": { "message": "hi" } }] } } })),
            record(1, "stream", json!("started")),
            record(2, "turns", json!({ "push_turn": { "output": [
                { "text": "hello" },
                { "tool_call": { "id": "c1", "name": "shell", "arguments": "{}" } }
            ] } })),
            record(3, "stream", json!({ "completed": { "usage": null, "stop_reason": "tool_calls" } })),
            record(4, "tool", json!({ "decided": { "tool_call_id": "c1", "decision": { "allowed": { "by": "user" } } } })),
            record(5, "tool", json!({ "finished": { "tool_call_id": "c1", "outcome": { "success": { "content": [{ "text": { "text": "ok" } }] } } } })),
        ]
    }

    fn fold_all(records: &[Value]) -> (Fold, Vec<ChatItem>) {
        let mut fold = Fold::default();
        let mut items = Vec::new();
        for record in records {
            fold.apply(&mut items, record);
        }
        (fold, items)
    }

    fn shape(items: &[ChatItem]) -> String {
        format!("{items:?}")
    }

    #[test]
    fn overlapping_records_are_ignored() {
        let records = history();
        let (mut fold, mut items) = fold_all(&records);
        let before = shape(&items);
        for record in &records[2..] {
            assert!(matches!(fold.apply(&mut items, record), Applied::Ignored));
        }
        assert_eq!(shape(&items), before);
    }

    #[test]
    fn streamed_items_are_replaced_by_the_recorded_turn() {
        let records = history();
        let (_, expected) = fold_all(&records[..2]);
        let mut fold = Fold::default();
        let mut items = Vec::new();
        for record in &records[..2] {
            fold.apply(&mut items, record);
        }
        // Live deltas for the response, before its turns record arrives.
        fold.open_tail(items.len());
        items.push(ChatItem::Assistant { text: "hel".into() });
        items.push(ChatItem::Assistant { text: "lo".into() });
        items.push(ChatItem::Tool {
            id: "c1".into(),
            name: "shell".into(),
            args: "{}".into(),
            result: None,
            decision: None,
        });
        fold.apply(&mut items, &records[2]);
        let (_, recorded) = fold_all(&records[..3]);
        assert_eq!(shape(&items), shape(&recorded));
        assert_eq!(items.len(), expected.len() + 2);
    }

    #[test]
    fn snapshot_and_live_fold_agree() {
        let records = history();
        let (_, snapshot) = fold_all(&records);
        // Live: the first two records arrive, then the rest overlap the snapshot.
        let mut fold = Fold::default();
        let mut live = Vec::new();
        for record in &records[..2] {
            fold.apply(&mut live, record);
        }
        for record in &records {
            fold.apply(&mut live, record);
        }
        assert_eq!(shape(&live), shape(&snapshot));
    }

    #[test]
    fn interrupted_partial_is_dropped_when_the_conversation_moves_on() {
        let mut records = history()[..2].to_vec();
        records.push(record(2, "stream", json!({ "interrupted": { "partial": [{ "text": "half" }] } })));
        let (mut fold, mut items) = fold_all(&records);
        assert!(matches!(items.last(), Some(ChatItem::Error { .. })));
        assert!(items.iter().any(|i| matches!(i, ChatItem::Assistant { text } if text == "half")));
        fold.apply(
            &mut items,
            &record(3, "turns", json!({ "push_entry": { "user": { "message": "again" } } })),
        );
        assert!(!items.iter().any(|i| matches!(i, ChatItem::Error { .. })));
        assert!(!items.iter().any(|i| matches!(i, ChatItem::Assistant { .. })));
        assert!(matches!(items.last(), Some(ChatItem::User { text }) if text == "again"));
    }

    #[test]
    fn pending_calls_are_the_last_output_calls_without_a_decision() {
        let records = history();
        let (fold, _) = fold_all(&records[..4]);
        assert_eq!(fold.pending(), vec!["c1".to_string()]);
        let (fold, _) = fold_all(&records);
        assert!(fold.pending().is_empty());
    }
}
