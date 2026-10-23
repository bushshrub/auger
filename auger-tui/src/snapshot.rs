//! Rebuild the transcript from a snapshot: the session's records as NDJSON.
//!
//! Records are folded the way auger-core restores a session: `turns`
//! records replay the conversation, `tool` records add outcomes and
//! decisions, and an interrupted stream's partial shows until the
//! conversation moves on.

use crate::types::ChatItem;
use crate::types::ToolDecision;
use serde_json::Value;
use std::collections::HashMap;

/// The transcript, plus the tool calls of the last response that have no
/// outcome yet, in order.
pub(crate) struct Transcript {
    pub(crate) items: Vec<ChatItem>,
    pub(crate) pending: Vec<String>,
}

pub(crate) fn rebuild(lines: &[String]) -> Transcript {
    let mut turns: Vec<Value> = Vec::new();
    let mut partial: Option<Value> = None;
    let mut outcomes: HashMap<String, String> = HashMap::new();
    let mut decisions: HashMap<String, ToolDecision> = HashMap::new();

    for line in lines {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let data = record.get("data").cloned().unwrap_or(Value::Null);
        match record.get("type").and_then(Value::as_str) {
            Some("turns") => {
                apply_change(&mut turns, data);
                partial = None;
            }
            Some("stream") => {
                partial = data
                    .get("interrupted")
                    .or_else(|| data.get("failed"))
                    .and_then(|payload| payload.get("partial"))
                    .filter(|partial| !partial.is_null())
                    .cloned();
            }
            Some("tool") => {
                if let Some(result) = data.get("finished") {
                    if let Some(id) = result.get("tool_call_id").and_then(Value::as_str) {
                        outcomes.insert(id.to_string(), outcome_text(result));
                        if result.pointer("/outcome/denied").is_some() {
                            decisions.entry(id.to_string()).or_insert(ToolDecision::Denied);
                        }
                    }
                } else if let Some(decided) = data.get("decided") {
                    record_decision(&mut decisions, decided);
                }
            }
            _ => {}
        }
    }

    let mut items = Vec::new();
    let mut last_calls = Vec::new();
    for turn in &turns {
        if let Some(entries) = turn.pointer("/input/entries").and_then(Value::as_array) {
            last_calls.clear();
            for entry in entries {
                push_entry(&mut items, &outcomes, entry);
            }
        } else if let Some(blocks) = turn.get("output").and_then(Value::as_array) {
            last_calls = push_blocks(&mut items, blocks);
        }
    }
    if let Some(blocks) = partial.as_ref().and_then(Value::as_array) {
        push_blocks(&mut items, blocks);
    }

    let mut pending = Vec::new();
    for item in &mut items {
        let ChatItem::Tool {
            id,
            result,
            decision,
            ..
        } = item
        else {
            continue;
        };
        if result.is_none() {
            *result = outcomes.get(id).cloned();
        }
        if decision.is_none() {
            *decision = decisions.get(id).cloned();
        }
        if result.is_none() && last_calls.contains(id) {
            pending.push(id.clone());
        } else if decision.is_none() {
            *decision = Some(ToolDecision::Approved);
        }
    }
    Transcript { items, pending }
}

fn apply_change(turns: &mut Vec<Value>, change: Value) {
    if let Some(turn) = change.get("push_turn") {
        turns.push(turn.clone());
    } else if let Some(entry) = change.get("push_entry") {
        if let Some(entries) = turns
            .last_mut()
            .and_then(|turn| turn.pointer_mut("/input/entries"))
            .and_then(Value::as_array_mut)
        {
            entries.push(entry.clone());
        }
    } else if let Some(len) = change.pointer("/truncate/len").and_then(Value::as_u64) {
        turns.truncate(len as usize);
    }
}

fn push_entry(items: &mut Vec<ChatItem>, outcomes: &HashMap<String, String>, entry: &Value) {
    if let Some(text) = entry.pointer("/user/message").and_then(Value::as_str) {
        items.push(ChatItem::User {
            text: text.to_string(),
        });
    } else if let Some(result) = entry.pointer("/harness/tool_result") {
        let Some(id) = result.get("tool_call_id").and_then(Value::as_str) else {
            return;
        };
        let text = outcomes.get(id).cloned().or_else(|| {
            result.get("content").and_then(Value::as_str).map(str::to_string)
        });
        for item in items.iter_mut() {
            if let ChatItem::Tool { id: tid, result, .. } = item {
                if tid == id {
                    *result = text.clone();
                }
            }
        }
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

fn record_decision(decisions: &mut HashMap<String, ToolDecision>, decided: &Value) {
    let Some(id) = decided.get("tool_call_id").and_then(Value::as_str) else {
        return;
    };
    let decision = decided.get("decision").unwrap_or(&Value::Null);
    let by = decision
        .get("allowed")
        .or_else(|| decision.get("denied"))
        .and_then(|d| d.get("by"));
    let policy = by
        .and_then(|by| by.get("policy"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let decision = match (decision.get("allowed").is_some(), policy) {
        (true, Some(node)) => ToolDecision::Auto(node),
        (true, None) => ToolDecision::Approved,
        (false, _) => ToolDecision::Denied,
    };
    decisions.insert(id.to_string(), decision);
}

/// The text of a `ToolCallResult`'s outcome.
fn outcome_text(result: &Value) -> String {
    let Some(outcome) = result.get("outcome") else {
        return String::new();
    };
    if outcome.as_str() == Some("interrupted") {
        return "Tool call was interrupted.".to_string();
    }
    if let Some(reason) = outcome.pointer("/denied/reason").and_then(Value::as_str) {
        return reason.to_string();
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
