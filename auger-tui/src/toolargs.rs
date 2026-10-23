//! Readable arguments for tool calls that have no file-shaped view (see
//! [`crate::diff`]).
//!
//! The raw JSON is unreadable once a command is longer than a few words, and
//! truncating it to one line hid exactly the part that matters when deciding
//! whether to approve a call: the command itself.

use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use serde_json::Value;

/// Rows shown for a collapsed call before the rest is folded away.
const COLLAPSED_ARG_LINES: usize = 3;
const COLLAPSED_RESULT_LINES: usize = 2;

/// How the arguments of a non-file tool call should read.
pub struct ArgView {
    /// Prefix for the first line, e.g. `$` for a shell command.
    pub marker: &'static str,
    /// The argument text, one entry per display line, never truncated.
    pub lines: Vec<String>,
    /// Highlight the lines as bash.
    pub shell: bool,
}

/// Turn raw tool arguments into displayable text.
///
/// Shell commands are shown verbatim, since that is the whole content of the
/// call. Everything else is pretty-printed JSON so nested arguments stay
/// readable. Arguments still streaming in are not valid JSON yet and are shown
/// as-is rather than dropped.
pub fn arg_view(name: &str, args: &str) -> ArgView {
    let parsed: Option<Value> = serde_json::from_str(args).ok();

    let command = match &parsed {
        Some(value) => value.get("command").and_then(Value::as_str).map(str::to_string),
        None => partial_field(args, "command"),
    };
    if let Some(command) = command.filter(|_| name == "shell") {
        return ArgView {
            marker: "$",
            lines: crate::shellfmt::format(&command),
            shell: true,
        };
    }

    let text = match &parsed {
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_else(|_| args.to_string()),
        None => args.to_string(),
    };
    ArgView {
        marker: " ",
        lines: text_lines(&text),
        shell: false,
    }
}

/// The string value of `field` in arguments that are still streaming, as far
/// as it has arrived: `{"command":"cd /tm` gives `cd /tm`.
pub fn partial_field(args: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let rest = args[args.find(&key)? + key.len()..].trim_start();
    let rest = rest.strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    Some(out)
}

/// Render a non-file tool call: its arguments, then its result if one has
/// arrived. `width` is the space available for text and `max_result_lines`
/// caps an expanded result so one noisy command can't bury the transcript.
pub fn render(
    name: &str,
    args: &str,
    result: Option<&str>,
    expanded: bool,
    width: usize,
    max_result_lines: usize,
) -> Vec<Line<'static>> {
    let view = arg_view(name, args);
    let mut out = Vec::new();

    let mut rows = Vec::new();
    for (index, line) in view.lines.iter().enumerate() {
        let marker = if index == 0 { view.marker } else { " " };
        let parts = crate::ui::wrap_text(line, width.saturating_sub(2));
        let spans: Vec<Vec<Span<'static>>> = match view.shell {
            true => crate::diff::highlight(&parts.join("\n"), "command.sh"),
            false => parts
                .into_iter()
                .map(|part| vec![Span::styled(part, Style::default().fg(Color::White))])
                .collect(),
        };
        for (wrapped_index, content) in spans.into_iter().enumerate() {
            let prefix = if wrapped_index == 0 { marker } else { " " };
            let mut row = vec![Span::styled(
                format!("{prefix} "),
                Style::default().fg(Color::Magenta),
            )];
            row.extend(content);
            rows.push(Line::from(row));
        }
    }
    let total = rows.len();
    let shown = match expanded {
        true => total,
        false => COLLAPSED_ARG_LINES.min(total),
    };
    out.extend(rows.into_iter().take(shown));
    if shown < total {
        out.push(hint(format!("... +{} more lines", total - shown)));
    }

    if let Some(result) = result {
        out.extend(result_lines(result, expanded, width, max_result_lines));
    }
    out
}

fn result_lines(result: &str, expanded: bool, width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let all: Vec<String> = result
        .lines()
        .flat_map(|line| crate::ui::wrap_text(line, width.saturating_sub(2)))
        .collect();
    let budget = match expanded {
        true => max_lines,
        false => COLLAPSED_RESULT_LINES,
    };

    let mut out: Vec<Line<'static>> = all
        .iter()
        .take(budget)
        .map(|line| {
            Line::from(vec![
                Span::styled("| ", Style::default().fg(Color::DarkGray)),
                Span::styled(line.clone(), Style::default().fg(Color::Gray)),
            ])
        })
        .collect();
    if all.len() > budget {
        let hidden = all.len() - budget;
        out.push(hint(match expanded {
            true => format!("... {hidden} more lines"),
            false => format!("... +{hidden} more lines"),
        }));
    }
    out
}

fn hint(text: String) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
}

/// Split into lines, keeping at least one entry so callers always have a
/// summary line to show.
fn text_lines(text: &str) -> Vec<String> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_fields_decode_as_far_as_they_arrived() {
        assert_eq!(partial_field(r#"{"command": "echo \"hi\nx"#, "command").as_deref(), Some("echo \"hi\nx"));
        assert_eq!(partial_field(r#"{"comm"#, "command"), None);
    }
}
