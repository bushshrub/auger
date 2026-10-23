//! Rendering of chat items into styled lines.
//!
//! Finished items are printed once into the terminal's own scrollback, so
//! [`final_lines`] must show everything worth keeping: nothing printed there
//! can be expanded or redrawn later. [`live_lines`] renders the items still
//! changing at the bottom of the screen.

use crate::types::ChatItem;
use crate::types::ToolDecision;
use crate::ui::wrap_text;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

/// Rows of a diff or file preview kept in scrollback.
const MAX_BODY_LINES: usize = 20;

/// What the live area needs to know about pending tool calls.
pub struct LiveTool<'a> {
    /// Every call still waiting for a decision.
    pub pending: &'a [String],
}

/// Lines printed into scrollback once `item` is finished. Starts with a
/// blank spacer row.
pub fn final_lines(item: &ChatItem, width: usize) -> Vec<Line<'static>> {
    lines_for(item, width, false)
}

/// Lines for the transcript pager: like [`final_lines`], with nothing folded.
pub fn transcript_lines(item: &ChatItem, width: usize) -> Vec<Line<'static>> {
    lines_for(item, width, true)
}

fn lines_for(item: &ChatItem, width: usize, expanded: bool) -> Vec<Line<'static>> {
    let body_limit = if expanded { 0 } else { MAX_BODY_LINES };
    let mut lines = vec![Line::from("")];
    match item {
        ChatItem::Reasoning { started, .. } => {
            let label = match started {
                Some(started) => format!("  thought for {}s", started.elapsed().as_secs().max(1)),
                None => "  thought".to_string(),
            };
            lines.push(Line::from(Span::styled(label, dim().add_modifier(Modifier::ITALIC))));
        }
        ChatItem::Tool {
            name,
            args,
            result,
            decision,
            ..
        } if name == "shell" => {
            lines.extend(crate::exec::lines(args, result.as_deref(), decision.as_ref(), width, expanded));
        }
        ChatItem::Tool {
            name,
            args,
            result,
            decision,
            ..
        } => {
            lines.push(tool_header(name, decision.as_ref()));
            match crate::diff::tool_view(name, args) {
                Some(view) => {
                    lines.push(Line::from(Span::styled(
                        format!("    {}", crate::diff::summary(&view, result.as_deref())),
                        dim(),
                    )));
                    lines.extend(indent(crate::diff::body(&view, result.as_deref(), body_limit)));
                }
                None => lines.extend(indent(crate::toolargs::render(
                    name,
                    args,
                    result.as_deref(),
                    expanded,
                    width.saturating_sub(4),
                    if expanded { usize::MAX } else { MAX_BODY_LINES },
                ))),
            }
        }
        other => lines.extend(plain_lines(other, width)),
    }
    lines
}

/// Lines for an item that is still changing, drawn in the live area.
pub fn live_lines(item: &ChatItem, width: usize, tools: &LiveTool) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("")];
    match item {
        ChatItem::Reasoning { text, .. } => {
            let style = dim().add_modifier(Modifier::ITALIC);
            for l in wrap_text(text, width.saturating_sub(2)) {
                lines.push(Line::from(Span::styled(format!("  {l}"), style)));
            }
        }
        ChatItem::Tool {
            id,
            name,
            args,
            result,
            decision,
        } => {
            // A call awaiting a decision is shown by the approval panel.
            if decision.is_none() && tools.pending.iter().any(|t| t == id) {
                return vec![];
            }
            if name == "shell" {
                lines.extend(crate::exec::lines(args, result.as_deref(), decision.as_ref(), width, false));
                return lines;
            }
            lines.push(tool_header(name, decision.as_ref()));
            lines.extend(indent(crate::toolargs::render(
                name,
                args,
                result.as_deref(),
                false,
                width.saturating_sub(4),
                MAX_BODY_LINES,
            )));
        }
        other => lines.extend(plain_lines(other, width)),
    }
    lines
}

fn plain_lines(item: &ChatItem, width: usize) -> Vec<Line<'static>> {
    match item {
        ChatItem::User { text } => wrap_text(text, width.saturating_sub(4))
            .into_iter()
            .map(|l| {
                Line::from(Span::styled(
                    format!("  > {l}"),
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ))
            })
            .collect(),
        ChatItem::Assistant { text } => {
            indent_by("  ", crate::markdown::render_markdown(text, width.saturating_sub(2)))
        }
        ChatItem::Error { text } => vec![Line::from(Span::styled(
            format!("  ! {text}"),
            Style::default().fg(Color::Red),
        ))],
        ChatItem::Reasoning { .. } | ChatItem::Tool { .. } => vec![],
    }
}

fn tool_header(name: &str, decision: Option<&ToolDecision>) -> Line<'static> {
    let green = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let red = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
    let (bullet, label) = match decision {
        Some(ToolDecision::Approved) => (green, "  \u{b7} approved".to_string()),
        Some(ToolDecision::Denied) => (red, "  \u{b7} denied".to_string()),
        Some(ToolDecision::Auto(node)) => (green, format!("  \u{b7} auto: {node}")),
        None => (dim(), String::new()),
    };
    Line::from(vec![
        Span::styled("\u{2022} ", bullet),
        Span::styled(name.to_string(), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(label, dim()),
    ])
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn indent(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    indent_by("    ", lines)
}

fn indent_by(prefix: &'static str, lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::raw(prefix)];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}
