//! Folding of exploration: consecutive calls that only read, search or list
//! collapse into one "Explored" cell, as Claude Code and Codex do.

use crate::types::ChatItem;
use crate::types::ToolDecision;
use auger_shell::Command;
use auger_shell::Joint;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use serde_json::Value;

/// Detail rows shown under the summary before folding.
const DETAIL_ROWS: usize = 4;

/// What one exploratory step did.
#[derive(Debug, PartialEq)]
pub enum Intent {
    Read(String),
    Search { pattern: String, path: Option<String> },
    List(String),
}

/// A stretch of chat items to render.
pub enum Segment<'a> {
    Item(&'a ChatItem),
    /// Two or more exploratory calls, with the reasoning between them dropped.
    Explored(Vec<Intent>),
}

/// How a tool item takes part in folding.
enum Role {
    /// Explores and succeeded: foldable.
    Explored(Vec<Intent>),
    /// No result yet; may still turn out to explore.
    Pending,
    /// Anything else ends a fold.
    Other,
}

fn role(item: &ChatItem) -> Role {
    let ChatItem::Tool {
        name,
        args,
        result,
        decision,
        ..
    } = item
    else {
        return Role::Other;
    };
    // Arguments may still be streaming, so an unfinished call might yet
    // explore.
    if result.is_none() {
        return Role::Pending;
    }
    let Some(intents) = intents(name, args) else {
        return Role::Other;
    };
    match result {
        None => Role::Pending,
        Some(_) if matches!(decision, Some(ToolDecision::Denied)) => Role::Other,
        Some(result) if succeeded(name, result) => Role::Explored(intents),
        Some(_) => Role::Other,
    }
}

/// Whether a finished exploratory call at `index` must stay live because the
/// next call may still join its fold.
pub fn holds(items: &[ChatItem], index: usize, idle: bool) -> bool {
    if !matches!(role(&items[index]), Role::Explored(_)) {
        return false;
    }
    match items[index + 1..]
        .iter()
        .find(|item| !filler(item))
    {
        None => !idle,
        Some(next) => match role(next) {
            Role::Pending => true,
            Role::Explored(_) => holds(items, index + 1 + position(items, index), idle),
            Role::Other => false,
        },
    }
}

/// Offset from `index + 1` to the next non-reasoning item.
fn position(items: &[ChatItem], index: usize) -> usize {
    items[index + 1..]
        .iter()
        .position(|item| !filler(item))
        .unwrap_or(0)
}

/// Items a fold absorbs: reasoning and blank text between calls.
fn filler(item: &ChatItem) -> bool {
    match item {
        ChatItem::Reasoning { .. } => true,
        ChatItem::Assistant { text } => text.trim().is_empty(),
        _ => false,
    }
}

/// Group `items` into segments, folding runs of two or more exploratory calls.
pub fn segments(items: &[ChatItem]) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let mut run = Vec::new();
        let mut end = i;
        let mut j = i;
        while j < items.len() {
            match role(&items[j]) {
                Role::Explored(intents) => {
                    run.push(intents);
                    end = j + 1;
                    j += 1;
                }
                _ if filler(&items[j]) && !run.is_empty() => j += 1,
                _ => break,
            }
        }
        if run.len() >= 2 {
            out.push(Segment::Explored(run.into_iter().flatten().collect()));
            i = end;
        } else {
            out.push(Segment::Item(&items[i]));
            i += 1;
        }
    }
    out
}

/// The folded cell: a summary line, then one row per step.
pub fn lines(intents: &[Intent], width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let mut out = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("\u{2022} ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(summary(intents), Style::default().add_modifier(Modifier::BOLD)),
        ]),
    ];
    let rows: Vec<String> = intents.iter().map(describe).collect();
    for (index, row) in rows.iter().take(DETAIL_ROWS).enumerate() {
        let prefix = if index == 0 { "  \u{2514} " } else { "    " };
        let row: String = row.chars().take(width.saturating_sub(4)).collect();
        out.push(Line::from(Span::styled(format!("{prefix}{row}"), dim)));
    }
    if rows.len() > DETAIL_ROWS {
        out.push(Line::from(Span::styled(
            format!("    \u{2026} +{} more", rows.len() - DETAIL_ROWS),
            dim,
        )));
    }
    out
}

/// "Read 3 files, searched 2 patterns, listed 1 directory".
fn summary(intents: &[Intent]) -> String {
    let count = |f: fn(&Intent) -> bool| intents.iter().filter(|i| f(i)).count();
    let reads = count(|i| matches!(i, Intent::Read(_)));
    let searches = count(|i| matches!(i, Intent::Search { .. }));
    let lists = count(|i| matches!(i, Intent::List(_)));
    let parts: Vec<String> = [
        (reads, "Read", "file", "files"),
        (searches, "searched", "pattern", "patterns"),
        (lists, "listed", "directory", "directories"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, verb, one, many)| format!("{verb} {n} {}", if n == 1 { one } else { many }))
    .collect();
    let mut text = parts.join(", ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    text
}

fn describe(intent: &Intent) -> String {
    match intent {
        Intent::Read(path) => format!("Read {path}"),
        Intent::Search { pattern, path: Some(path) } => format!("Search {pattern} in {path}"),
        Intent::Search { pattern, path: None } => format!("Search {pattern}"),
        Intent::List(path) => format!("List {path}"),
    }
}

fn succeeded(name: &str, result: &str) -> bool {
    match name {
        "read_file" => result.starts_with("Total lines:"),
        _ => serde_json::from_str::<Value>(result)
            .ok()
            .and_then(|v| v.get("exit_code").and_then(Value::as_i64))
            == Some(0),
    }
}

/// The exploratory steps of a call, or `None` if it does anything else.
fn intents(name: &str, args: &str) -> Option<Vec<Intent>> {
    let args: Value = serde_json::from_str(args).ok()?;
    match name {
        "read_file" => Some(vec![Intent::Read(short(args.get("path")?.as_str()?))]),
        "shell" => {
            let commands = auger_shell::plain_commands(args.get("command")?.as_str()?)?;
            let mut out = Vec::new();
            for command in &commands {
                out.extend(command_intents(command)?);
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

/// Filters that only shape another command's output.
const FILTERS: &[&str] = &["head", "tail", "wc", "sort", "uniq", "cut", "tr", "grep", "cat"];
/// Commands that print nothing worth folding on their own.
const QUIET: &[&str] = &["cd", "echo", "printf", "true", "pwd"];

fn command_intents(command: &Command) -> Option<Vec<Intent>> {
    let program = command.argv[0].text.as_str();
    let operands: Vec<&str> = command.argv[1..]
        .iter()
        .map(|a| a.text.as_str())
        .filter(|a| !a.starts_with('-') && a.parse::<f64>().is_err())
        .collect();
    let writes = command
        .redirects
        .iter()
        .any(|r| r.op.contains('>') && !r.op.contains('&') && r.target.text != "/dev/null");
    if writes {
        return None;
    }
    let piped = command.joint == Some(Joint::Pipe);
    if QUIET.contains(&program) || (piped && FILTERS.contains(&program)) {
        return Some(Vec::new());
    }
    Some(match program {
        "cat" | "head" | "tail" | "nl" | "wc" => {
            operands.iter().map(|p| Intent::Read(short(p))).collect()
        }
        "grep" | "egrep" | "rg" => {
            let (pattern, paths) = operands.split_first()?;
            vec![Intent::Search {
                pattern: pattern.to_string(),
                path: paths.first().map(|p| short(p)),
            }]
        }
        "ls" | "tree" | "find" => vec![Intent::List(short(operands.first().unwrap_or(&".")))],
        _ => return None,
    })
}

/// A path as shown in a detail row: its last two components.
fn short(path: &str) -> String {
    let parts: Vec<&str> = path.trim_end_matches('/').rsplit('/').take(2).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> Option<Vec<Intent>> {
        intents("shell", &serde_json::json!({ "command": command }).to_string())
    }

    #[test]
    fn classifies_reads_searches_and_lists() {
        assert_eq!(
            shell("cd /p && cat src/a.rs | head -20 && grep -rn todo src && ls"),
            Some(vec![
                Intent::Read("src/a.rs".into()),
                Intent::Search { pattern: "todo".into(), path: Some("src".into()) },
                Intent::List(".".into()),
            ])
        );
    }

    #[test]
    fn anything_else_is_not_exploration() {
        assert_eq!(shell("cargo test 2>&1 | tail -40"), None);
        assert_eq!(shell("cat a > b"), None);
    }
}
