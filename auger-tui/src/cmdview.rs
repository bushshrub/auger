//! Display rows for a shell command: formatted lines, highlighted, numbered
//! when there is more than one, and word-wrapped with a hanging indent so a
//! long line stays readable.

use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Span;

/// Extra indent for the continuation rows of a wrapped line.
const HANG: usize = 2;

/// The rows of the command in `args`, fitted to `width`. The second value
/// says whether the rows carry line numbers.
pub fn rows(args: &str, width: usize) -> (Vec<Vec<Span<'static>>>, bool) {
    let lines = crate::toolargs::arg_view("shell", args).lines;
    let numbered = lines.len() > 1;
    let gutter = if numbered { lines.len().to_string().len() + 1 } else { 0 };
    let highlighted = crate::diff::highlight(&lines.join("\n"), "command.sh");

    let mut out = Vec::new();
    for (index, (line, spans)) in lines.iter().zip(highlighted).enumerate() {
        let indent = line.len() - line.trim_start().len();
        let breaks = wrap_points(line, width.saturating_sub(gutter).max(1), indent + HANG);
        let mut pieces = split_spans(spans, &breaks).into_iter();
        let number = Span::styled(
            format!("{:>w$} ", index + 1, w = gutter.saturating_sub(1)),
            Style::default().fg(Color::DarkGray),
        );
        let mut first = Vec::new();
        if numbered {
            first.push(number);
        }
        first.extend(pieces.next().unwrap_or_default());
        out.push(first);
        for piece in pieces {
            let mut row = vec![Span::raw(" ".repeat(gutter + indent + HANG))];
            row.extend(piece);
            out.push(row);
        }
    }
    (out, numbered)
}

/// Char offsets where `line` breaks to fit `width`, preferring spaces.
/// Continuation rows lose `hang` columns to their indent.
fn wrap_points(line: &str, width: usize, hang: usize) -> Vec<usize> {
    let chars: Vec<char> = line.chars().collect();
    let mut breaks = Vec::new();
    let mut start = 0;
    let mut room = width;
    while chars.len() - start > room {
        let limit = start + room;
        let cut = (start + 1..=limit)
            .rev()
            .find(|&i| chars[i - 1] == ' ')
            .unwrap_or(limit);
        breaks.push(cut);
        start = cut;
        room = width.saturating_sub(hang).max(10);
    }
    breaks
}

/// Cut a row of spans at the char offsets in `breaks`.
fn split_spans(spans: Vec<Span<'static>>, breaks: &[usize]) -> Vec<Vec<Span<'static>>> {
    let mut rows = vec![Vec::new()];
    let mut offset = 0;
    let mut next = breaks.iter().copied().peekable();
    for span in spans {
        let mut text: &str = &span.content;
        let mut owned = Vec::new();
        while let Some(&cut) = next.peek() {
            let len = text.chars().count();
            if cut >= offset + len {
                break;
            }
            let at = text.char_indices().nth(cut - offset).map_or(text.len(), |(i, _)| i);
            owned.push((text[..at].to_string(), true));
            text = &text[at..];
            offset = cut;
            next.next();
        }
        owned.push((text.to_string(), false));
        offset += text.chars().count();
        for (piece, ends_row) in owned {
            if !piece.is_empty() {
                rows.last_mut().unwrap().push(Span::styled(piece, span.style));
            }
            if ends_row {
                rows.push(Vec::new());
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_spaces_with_room_for_the_hang() {
        assert_eq!(wrap_points("aaa bbb ccc", 8, 2), [8]);
        assert_eq!(wrap_points("aaa bbb ccc ddd", 8, 2), [8]);
        assert_eq!(wrap_points("short", 8, 2), Vec::<usize>::new());
    }
}
