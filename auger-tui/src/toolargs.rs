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

/// Lines shown for a collapsed call before the rest is folded away.
const COLLAPSED_ARG_LINES: usize = 3;
const COLLAPSED_RESULT_LINES: usize = 2;

/// How the arguments of a non-file tool call should read.
pub struct ArgView {
    /// Prefix for the first line, e.g. `$` for a shell command.
    pub marker: &'static str,
    /// The argument text, one entry per source line, never truncated.
    pub lines: Vec<String>,
}

/// Turn raw tool arguments into displayable text.
///
/// Shell commands are shown verbatim, since that is the whole content of the
/// call. Everything else is pretty-printed JSON so nested arguments stay
/// readable. Arguments still streaming in are not valid JSON yet and are shown
/// as-is rather than dropped.
pub fn arg_view(name: &str, args: &str) -> ArgView {
    let parsed: Option<Value> = serde_json::from_str(args).ok();

    if let Some(command) = parsed
        .as_ref()
        .filter(|_| name == "shell")
        .and_then(|v| v.get("command"))
        .and_then(Value::as_str)
    {
        return ArgView {
            marker: "$",
            lines: text_lines(command),
        };
    }

    let text = match &parsed {
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_else(|_| args.to_string()),
        None => args.to_string(),
    };
    ArgView {
        marker: " ",
        lines: text_lines(&text),
    }
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

    let shown = match expanded {
        true => view.lines.len(),
        false => COLLAPSED_ARG_LINES.min(view.lines.len()),
    };
    for (index, line) in view.lines.iter().take(shown).enumerate() {
        let marker = if index == 0 { view.marker } else { " " };
        for (wrapped_index, part) in crate::ui::wrap_text(line, width.saturating_sub(2))
            .into_iter()
            .enumerate()
        {
            let prefix = if wrapped_index == 0 { marker } else { " " };
            out.push(Line::from(vec![
                Span::styled(format!("{prefix} "), Style::default().fg(Color::Magenta)),
                Span::styled(part, Style::default().fg(Color::White)),
            ]));
        }
    }
    if shown < view.lines.len() {
        out.push(hint(format!(
            "+{} more lines - click to expand",
            view.lines.len() - shown
        )));
    }

    if let Some(result) = result {
        out.extend(result_lines(result, expanded, width, max_result_lines));
    } else if !expanded {
        out.push(hint("click to expand".to_string()));
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
            false => format!("+{hidden} more lines - click to expand"),
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
    fn shell_commands_show_verbatim_without_json_noise() {
        let view = arg_view("shell", r#"{"command":"cd /tmp && git status"}"#);
        assert_eq!(view.marker, "$");
        assert_eq!(view.lines, ["cd /tmp && git status"]);
    }

    /// The reported bug: a long command was cut at 80 characters, so the part
    /// being approved was invisible.
    #[test]
    fn long_commands_are_not_truncated() {
        let command = format!("git commit -m '{}'", "x".repeat(300));
        let args = serde_json::json!({ "command": command }).to_string();
        let view = arg_view("shell", &args);
        assert_eq!(view.lines[0], command);
    }

    #[test]
    fn multiline_commands_keep_their_lines() {
        let args = serde_json::json!({ "command": "set -e\nmake build\nmake test" }).to_string();
        let view = arg_view("shell", &args);
        assert_eq!(view.lines, ["set -e", "make build", "make test"]);
    }

    #[test]
    fn other_tools_get_pretty_printed_json() {
        let view = arg_view("grep", r#"{"pattern":"fn main","path":"src"}"#);
        assert!(view.lines.len() > 1, "pretty JSON spans lines");
        assert!(view.lines.iter().any(|l| l.contains("\"pattern\"")));
    }

    #[test]
    fn partial_arguments_are_shown_as_typed() {
        // Still streaming: not valid JSON, but the user should see it anyway.
        let view = arg_view("shell", "{\"command\":\"cd /tm");
        assert_eq!(view.lines, ["{\"command\":\"cd /tm"]);
    }

    #[test]
    fn empty_arguments_still_have_a_summary_line() {
        assert_eq!(arg_view("shell", "").lines, [""]);
    }

    // ── rendering ────────────────────────────────────────────────────────

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_wide_command_wraps_instead_of_being_cut_off() {
        let command = format!("git log {}", "-q".repeat(60));
        let args = serde_json::json!({ "command": command }).to_string();
        let rendered = text(&render("shell", &args, None, false, 40, 10));
        // Every character of the command survives the wrap.
        assert!(rendered.lines().count() > 1, "long command must wrap");
        assert_eq!(
            rendered.chars().filter(|c| *c == 'q').count(),
            60,
            "no characters dropped"
        );
    }

    #[test]
    fn a_collapsed_call_folds_a_long_command_but_says_so() {
        let command = (0..10)
            .map(|i| format!("step{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let args = serde_json::json!({ "command": command }).to_string();
        let rendered = text(&render("shell", &args, None, false, 60, 10));
        assert!(rendered.contains("step0"));
        assert!(!rendered.contains("step9"));
        assert!(rendered.contains("+7 more lines"), "got:\n{rendered}");
    }

    #[test]
    fn an_expanded_call_shows_the_whole_command() {
        let command = (0..10)
            .map(|i| format!("step{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let args = serde_json::json!({ "command": command }).to_string();
        let rendered = text(&render("shell", &args, None, true, 60, 10));
        assert!(rendered.contains("step9"));
        assert!(!rendered.contains("more lines"));
    }

    #[test]
    fn results_are_previewed_collapsed_and_capped_expanded() {
        let result = (0..30)
            .map(|i| format!("out{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let args = serde_json::json!({ "command": "ls" }).to_string();

        let collapsed = text(&render("shell", &args, Some(&result), false, 60, 10));
        assert!(collapsed.contains("out0"));
        assert!(!collapsed.contains("out5"));

        let expanded = text(&render("shell", &args, Some(&result), true, 60, 10));
        assert!(expanded.contains("out9"));
        assert!(expanded.contains("20 more lines"), "cap is reported");
    }
}
