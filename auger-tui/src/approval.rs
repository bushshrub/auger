//! The tool approval panel, modelled on Codex's: a bold question, what the
//! call will do, a numbered list of choices with the highlighted row in
//! reverse video, and a key hint footer.

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

/// Rows the panel spends on everything but the preview: question, choices,
/// footer and three spacers.
fn chrome_rows(queued: usize) -> usize {
    5 + choices(queued).len()
}

/// Rows scrolled by one page.
pub const PAGE_ROWS: usize = 5;

/// A choice in the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Choice {
    Approve,
    Deny,
}

impl Choice {
    /// The shortcut shown in parentheses.
    fn key(self) -> &'static str {
        match self {
            Choice::Approve => "y",
            Choice::Deny => "esc",
        }
    }

    pub fn approved(self) -> bool {
        matches!(self, Choice::Approve)
    }

    fn label(self) -> &'static str {
        match self {
            Choice::Approve => "Yes, proceed",
            Choice::Deny => "No, and tell auger what to do differently",
        }
    }
}

/// Choices offered when `queued` calls wait for a decision. Calls are
/// decided one at a time, since a call can't be approved unseen; denying one
/// skips the rest of its batch.
pub fn choices(queued: usize) -> Vec<Choice> {
    match queued {
        0 => vec![],
        _ => vec![Choice::Approve, Choice::Deny],
    }
}

/// Panel lines for the call `name(args)`, with row `selected` highlighted and
/// the preview scrolled down by `scroll` rows. The panel fits in `height`
/// rows; the preview gets what the rest leaves.
pub fn lines(
    name: &str,
    args: &str,
    queued: usize,
    selected: usize,
    scroll: usize,
    note: &str,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().add_modifier(Modifier::DIM);

    let mut header = vec![Span::styled(format!("  {}", question(name)), bold)];
    if queued > 1 {
        header.push(Span::styled(format!("  ({} more queued)", queued - 1), dim));
    }
    let mut out = vec![Line::from(header)];
    out.push(Line::from(""));
    let preview = preview(name, args, width);
    let rows = height.saturating_sub(chrome_rows(queued)).max(1);
    let total = preview.len();
    let scrolls = total > rows;
    let scroll = scroll.min(total.saturating_sub(rows));
    out.extend(preview.into_iter().skip(scroll).take(rows));
    out.push(Line::from(""));
    for (i, choice) in choices(queued).into_iter().enumerate() {
        let marker = if i == selected { "\u{203a} " } else { "  " };
        if choice == Choice::Deny && i == selected {
            // Focused, this row is a text box for what to do instead.
            let mut row = vec![Span::styled(format!("{marker}{}. ", i + 1), bold)];
            match note.is_empty() {
                true => row.push(Span::styled("Tell auger what to do differently", dim)),
                false => row.push(Span::raw(note.to_string())),
            }
            row.push(Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)));
            out.push(Line::from(row));
            continue;
        }
        let line = Line::from(vec![
            Span::raw(format!("{marker}{}. {} (", i + 1, choice.label())),
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
    if scrolls {
        if let Some(footer) = out.last_mut() {
            footer.spans.push(Span::styled(", ", dim));
            footer.spans.push(Span::styled("shift+\u{2191}\u{2193}", bold));
            footer.spans.push(Span::styled(
                format!(" to scroll ({}-{} of {total})", scroll + 1, (scroll + rows).min(total)),
                dim,
            ));
        }
    }
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
    let lines = match (name, parsed.get("command").and_then(|c| c.as_str())) {
        ("shell", Some(_)) => {
            let (rows, numbered) = crate::cmdview::rows(args, width.saturating_sub(4));
            rows.into_iter()
                .enumerate()
                .map(|(i, spans)| {
                    let prompt = if i == 0 && !numbered { "  $ " } else { "  " };
                    let mut row = vec![Span::styled(prompt, Style::default().fg(Color::Blue))];
                    row.extend(spans);
                    Line::from(row)
                })
                .collect()
        }
        _ => match crate::diff::tool_view(name, args) {
            Some(view) => {
                let mut lines = vec![Line::from(format!("  {}", crate::diff::summary(&view, None)))];
                lines.extend(indent(crate::diff::body(&view, None, 0)));
                lines
            }
            None => indent(crate::toolargs::render(
                name,
                args,
                None,
                true,
                width.saturating_sub(2),
                usize::MAX,
            )),
        },
    };
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
