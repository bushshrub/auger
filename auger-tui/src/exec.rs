//! Codex-style rendering of shell calls: a status bullet with the command,
//! one chain step per line, and a dim output preview.

use crate::types::ToolDecision;
use crate::ui::wrap_text;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use serde_json::Value;

/// Command rows shown below the header before folding.
const COMMAND_ROWS: usize = 2;
/// Output rows shown before folding.
const OUTPUT_ROWS: usize = 3;
/// Width of the header bullet and title, and of the row prefixes.
const HEADER_WIDTH: usize = 6;
const PREFIX_WIDTH: usize = 4;

enum Status {
    Running,
    Ok,
    Failed,
    Denied,
}

/// What a finished call printed.
struct Output {
    exit_code: Option<i64>,
    stdout: String,
    stderr: String,
}

/// The command as highlighted rows, one chain step per line, wrapped to
/// `width`.
pub fn command_rows(args: &str, width: usize) -> Vec<Vec<Span<'static>>> {
    let view = crate::toolargs::arg_view("shell", args);
    let rows: Vec<String> = view
        .lines
        .iter()
        .flat_map(|line| wrap_text(line, width.max(1)))
        .collect();
    crate::diff::highlight(&rows.join("\n"), "command.sh")
}

pub fn lines(
    args: &str,
    result: Option<&str>,
    decision: Option<&ToolDecision>,
    width: usize,
) -> Vec<Line<'static>> {
    let (status, output) = outcome(result, decision);
    let (bullet, title) = match status {
        Status::Running => (dim(), "Running"),
        Status::Ok => (Style::default().fg(Color::Green).add_modifier(Modifier::BOLD), "Ran"),
        Status::Failed => (Style::default().fg(Color::Red).add_modifier(Modifier::BOLD), "Ran"),
        Status::Denied => (Style::default().fg(Color::Red).add_modifier(Modifier::BOLD), "Denied"),
    };

    let mut highlighted = command_rows(args, width.saturating_sub(HEADER_WIDTH)).into_iter();

    let mut header = vec![
        Span::styled("\u{2022} ", bullet),
        Span::styled(format!("{title} "), Style::default().add_modifier(Modifier::BOLD)),
    ];
    header.extend(highlighted.next().unwrap_or_default());
    match decision {
        Some(ToolDecision::Auto(node)) => header.push(Span::styled(format!("  \u{b7} auto: {node}"), dim())),
        Some(ToolDecision::Approved) => header.push(Span::styled("  \u{b7} approved", dim())),
        _ => {}
    }
    if let Some(code) = output.as_ref().and_then(|o| o.exit_code).filter(|c| *c != 0) {
        header.push(Span::styled(format!("  \u{b7} exit {code}"), Style::default().fg(Color::Red)));
    }
    let mut out = vec![Line::from(header)];

    let rest: Vec<_> = highlighted.collect();
    let shown = fold(rest.len(), COMMAND_ROWS);
    let hidden = rest.len() - shown;
    for spans in rest.into_iter().take(shown) {
        let mut row = vec![Span::styled("  \u{2502} ", dim())];
        row.extend(spans);
        out.push(Line::from(row));
    }
    if hidden > 0 {
        out.push(Line::from(Span::styled(format!("  \u{2502} \u{2026} +{hidden} lines"), dim())));
    }

    if let Some(output) = output {
        let wrap = |text: &str, style: Style| -> Vec<(String, Style)> {
            text.lines()
                .flat_map(|line| wrap_text(line, width.saturating_sub(PREFIX_WIDTH).max(1)))
                .map(|row| (row, style))
                .collect()
        };
        // Errors first, so a failure is not folded away behind stdout.
        let mut rows = wrap(&output.stderr, Style::default().fg(Color::Red));
        rows.extend(wrap(&output.stdout, dim()));
        if rows.is_empty() {
            out.push(Line::from(Span::styled("  \u{2514} (no output)", dim())));
        }
        let shown = fold(rows.len(), OUTPUT_ROWS);
        for (index, (row, style)) in rows.iter().take(shown).enumerate() {
            let prefix = if index == 0 { "  \u{2514} " } else { "    " };
            out.push(Line::from(vec![
                Span::styled(prefix, dim()),
                Span::styled(row.clone(), *style),
            ]));
        }
        if rows.len() > shown {
            out.push(Line::from(Span::styled(
                format!("    \u{2026} +{} lines", rows.len() - shown),
                dim(),
            )));
        }
    }
    out
}

/// The status and display text of a call. The shell tool returns JSON with
/// `exit_code`, `stdout` and `stderr`; anything else (a denial, a tool error)
/// is shown as-is.
fn outcome(result: Option<&str>, decision: Option<&ToolDecision>) -> (Status, Option<Output>) {
    let Some(result) = result else {
        return (Status::Running, None);
    };
    let plain = |status| {
        let output = Output {
            exit_code: None,
            stdout: result.to_string(),
            stderr: String::new(),
        };
        (status, Some(output))
    };
    if matches!(decision, Some(ToolDecision::Denied)) {
        return plain(Status::Denied);
    }
    let Ok(value) = serde_json::from_str::<Value>(result) else {
        return plain(Status::Failed);
    };
    let field = |name: &str| {
        let text = value.get(name).and_then(Value::as_str).unwrap_or("");
        text.trim_end().to_string()
    };
    let exit_code = value.get("exit_code").and_then(Value::as_i64);
    let status = match exit_code {
        Some(0) => Status::Ok,
        _ => Status::Failed,
    };
    let output = Output {
        exit_code,
        stdout: field("stdout"),
        stderr: field("stderr"),
    };
    (status, Some(output))
}

/// Rows to show of `total` with room for `limit`: folding a single row would
/// only replace it with a hint of the same height.
fn fold(total: usize, limit: usize) -> usize {
    if total <= limit + 1 { total } else { limit }
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}
