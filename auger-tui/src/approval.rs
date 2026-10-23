//! The tool approval panel, modelled on Codex's: a bold question, what the
//! call will do, a numbered list of choices with the highlighted row in
//! reverse video, and a key hint footer.

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

/// Rows of call preview (command, diff, arguments) the panel shows.
const MAX_PREVIEW_LINES: usize = 8;

/// A choice in the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    Approve,
    ApproveAll,
    Deny,
    DenyAll,
}

impl Choice {
    /// The shortcut shown in parentheses.
    fn key(self) -> &'static str {
        match self {
            Choice::Approve => "y",
            Choice::ApproveAll => "a",
            Choice::Deny => "esc",
            Choice::DenyAll => "d",
        }
    }

    pub fn approved(self) -> bool {
        matches!(self, Choice::Approve | Choice::ApproveAll)
    }

    pub fn all(self) -> bool {
        matches!(self, Choice::ApproveAll | Choice::DenyAll)
    }

    fn label(self, queued: usize) -> String {
        match self {
            Choice::Approve => "Yes, proceed".to_string(),
            Choice::ApproveAll => format!("Yes, and approve all {queued} pending calls"),
            Choice::Deny => "No, and tell auger what to do differently".to_string(),
            Choice::DenyAll => format!("No to all {queued} pending calls"),
        }
    }
}

/// Choices offered when `queued` calls wait for a decision. The bulk choices
/// only appear for a batch.
pub fn choices(queued: usize) -> Vec<Choice> {
    match queued {
        0 => vec![],
        1 => vec![Choice::Approve, Choice::Deny],
        _ => vec![Choice::Approve, Choice::ApproveAll, Choice::Deny, Choice::DenyAll],
    }
}

/// Panel lines for the call `name(args)`, with row `selected` highlighted.
pub fn lines(name: &str, args: &str, queued: usize, selected: usize, width: usize) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    let mut out = vec![Line::from(Span::styled(format!("  {}", question(name)), bold))];
    out.push(Line::from(""));
    out.extend(preview(name, args, width));
    out.push(Line::from(""));
    for (i, choice) in choices(queued).into_iter().enumerate() {
        let marker = if i == selected { "\u{203a} " } else { "  " };
        let line = Line::from(vec![
            Span::raw(format!("{marker}{}. {} (", i + 1, choice.label(queued))),
            Span::styled(choice.key(), bold),
            Span::raw(")"),
        ]);
        out.push(match i == selected {
            true => line.style(bold.add_modifier(Modifier::REVERSED)),
            false => line,
        });
    }
    out.push(Line::from(""));
    out.push(Line::from(vec![
        Span::styled("  Press ", dim),
        Span::styled("enter", bold),
        Span::styled(" to confirm or ", dim),
        Span::styled("esc", bold),
        Span::styled(" to cancel", dim),
    ]));
    out
}

fn question(name: &str) -> String {
    match name {
        "shell" => "Would you like to run the following command?".to_string(),
        "write_file" | "edit_file" => "Would you like to make the following edits?".to_string(),
        other => format!("Would you like to allow {other}?"),
    }
}

/// What the call will do: the command for shell, a diff for edits, the
/// arguments otherwise.
fn preview(name: &str, args: &str, width: usize) -> Vec<Line<'static>> {
    let parsed: serde_json::Value = serde_json::from_str(args).unwrap_or_default();
    let mut lines = match (name, parsed.get("command").and_then(|c| c.as_str())) {
        ("shell", Some(command)) => command
            .lines()
            .enumerate()
            .map(|(i, l)| {
                let prompt = if i == 0 { "  $ " } else { "    " };
                Line::from(vec![
                    Span::styled(prompt, Style::default().fg(Color::Blue)),
                    Span::raw(l.to_string()),
                ])
            })
            .collect(),
        _ => match crate::diff::tool_view(name, args) {
            Some(view) => {
                let mut lines = vec![Line::from(format!("  {}", crate::diff::summary(&view, None)))];
                lines.extend(indent(crate::diff::body(&view, None, MAX_PREVIEW_LINES)));
                lines
            }
            None => indent(crate::toolargs::render(
                name,
                args,
                None,
                true,
                width.saturating_sub(2),
                MAX_PREVIEW_LINES,
            )),
        },
    };
    if lines.len() > MAX_PREVIEW_LINES {
        let more = lines.len() - MAX_PREVIEW_LINES;
        lines.truncate(MAX_PREVIEW_LINES);
        lines.push(Line::from(Span::styled(
            format!("  ... {more} more lines"),
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    lines
}

fn indent(lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    }

    #[test]
    fn bulk_choices_only_for_a_batch() {
        assert_eq!(choices(1), [Choice::Approve, Choice::Deny]);
        assert_eq!(choices(3).len(), 4);
    }

    #[test]
    fn a_shell_call_reads_like_codex() {
        let lines = text(&lines("shell", r#"{"command":"echo hi"}"#, 1, 0, 80));
        assert_eq!(
            lines,
            [
                "  Would you like to run the following command?",
                "",
                "  $ echo hi",
                "",
                "\u{203a} 1. Yes, proceed (y)",
                "  2. No, and tell auger what to do differently (esc)",
                "",
                "  Press enter to confirm or esc to cancel",
            ]
        );
    }
}
