//! Rendering for file-touching tool calls: unified diffs and file previews,
//! with syntax highlighting driven by the file extension.
//!
//! Only the tools that read or change files get this treatment. Everything
//! else falls back to a plain argument summary in [`crate::ui`].

use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use serde_json::Value;
use similar::ChangeTag;
use similar::TextDiff;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::FontStyle;
use syntect::highlighting::Theme;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxReference;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Loading the syntax and theme sets takes ~50ms and allocates a few MB, so it
/// happens once on first use rather than per rendered frame.
struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

fn highlighter() -> &'static Highlighter {
    static HIGHLIGHTER: OnceLock<Highlighter> = OnceLock::new();
    HIGHLIGHTER.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .get("base16-eighties.dark")
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default();
        Highlighter {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            theme,
        }
    })
}

fn syntax_for(path: &str) -> &'static SyntaxReference {
    let highlighter = highlighter();
    let extension = path.rsplit('.').next().unwrap_or("");
    highlighter
        .syntaxes
        .find_syntax_by_extension(extension)
        .unwrap_or_else(|| highlighter.syntaxes.find_syntax_plain_text())
}

fn to_ratatui_color(color: syntect::highlighting::Color) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

/// Highlight `text`, returning one span vector per line (newlines stripped).
fn highlight(text: &str, path: &str) -> Vec<Vec<Span<'static>>> {
    let highlighter = highlighter();
    let mut state = HighlightLines::new(syntax_for(path), &highlighter.theme);

    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        let spans = match state.highlight_line(line, &highlighter.syntaxes) {
            Ok(ranges) => ranges
                .into_iter()
                .map(|(style, piece)| {
                    let mut s = Style::default().fg(to_ratatui_color(style.foreground));
                    if style.font_style.contains(FontStyle::BOLD) {
                        s = s.add_modifier(Modifier::BOLD);
                    }
                    if style.font_style.contains(FontStyle::ITALIC) {
                        s = s.add_modifier(Modifier::ITALIC);
                    }
                    Span::styled(piece.trim_end_matches(['\n', '\r']).to_string(), s)
                })
                .filter(|s| !s.content.is_empty())
                .collect(),
            // A grammar failure shouldn't lose the line; fall back to plain.
            Err(_) => vec![Span::raw(line.trim_end_matches(['\n', '\r']).to_string())],
        };
        out.push(spans);
    }
    out
}

// ── tool argument shapes ─────────────────────────────────────────────────────

/// What a file-touching tool call is going to show.
pub enum ToolView {
    /// `edit_file`: a real before/after diff.
    Diff {
        path: String,
        old: String,
        new: String,
    },
    /// `write_file`: the whole file, shown as additions.
    Write { path: String, content: String },
    /// `read_file`: the file content, from the tool result.
    Read { path: String },
}

/// Classify a tool call. Returns `None` for tools with nothing file-shaped to
/// show, and for calls whose arguments haven't finished streaming yet.
pub fn tool_view(name: &str, args: &str) -> Option<ToolView> {
    let parsed: Value = serde_json::from_str(args).ok()?;
    let field = |key: &str| parsed.get(key).and_then(Value::as_str).map(str::to_string);
    let path = field("path")?;

    match name {
        "edit_file" => Some(ToolView::Diff {
            path,
            old: field("old_string")?,
            new: field("new_string")?,
        }),
        "write_file" => Some(ToolView::Write {
            path,
            content: field("content")?,
        }),
        "read_file" => Some(ToolView::Read { path }),
        _ => None,
    }
}

/// One-line summary shown when the tool call is collapsed.
pub fn summary(view: &ToolView, result: Option<&str>) -> String {
    match view {
        ToolView::Diff { path, old, new } => {
            let (added, removed) = counts(old, new);
            format!("{path} (+{added} −{removed})")
        }
        ToolView::Write { path, content } => {
            format!("{path} ({} lines)", content.lines().count())
        }
        ToolView::Read { path } => match result {
            Some(content) => format!("{path} ({} lines)", content.lines().count()),
            None => path.clone(),
        },
    }
}

fn counts(old: &str, new: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

// ── expanded rendering ───────────────────────────────────────────────────────

fn added_bg() -> Style {
    Style::default().bg(Color::Rgb(20, 48, 28))
}

fn removed_bg() -> Style {
    Style::default().bg(Color::Rgb(58, 24, 26))
}

/// Full body of an expanded tool call: a diff, a file preview, or nothing.
///
/// `max_lines` caps the output; the caller supplies the pane height so a huge
/// file can't take over the scroll buffer.
pub fn body(view: &ToolView, result: Option<&str>, max_lines: usize) -> Vec<Line<'static>> {
    let lines = match view {
        ToolView::Diff { path, old, new } => diff_lines(path, old, new),
        ToolView::Write { path, content } => {
            let mut out = Vec::new();
            for spans in highlight(content, path) {
                out.push(gutter_line("+", added_bg(), spans));
            }
            out
        }
        ToolView::Read { path } => match result {
            Some(content) => highlight(content, path)
                .into_iter()
                .map(|spans| gutter_line(" ", Style::default(), spans))
                .collect(),
            None => vec![],
        },
    };

    truncate(lines, max_lines)
}

fn truncate(mut lines: Vec<Line<'static>>, max_lines: usize) -> Vec<Line<'static>> {
    if max_lines == 0 || lines.len() <= max_lines {
        return lines;
    }
    let hidden = lines.len() - max_lines;
    lines.truncate(max_lines);
    lines.push(Line::from(Span::styled(
        format!("  … {hidden} more lines"),
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

/// Build one diff/preview row: a marker column, then the highlighted content,
/// with the row's background applied underneath the syntax colors.
fn gutter_line(marker: &str, background: Style, content: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{marker} "),
        background.fg(Color::DarkGray),
    )];
    for span in content {
        // Patch the background in while keeping syntect's foreground.
        let style = match background.bg {
            Some(bg) => span.style.bg(bg),
            None => span.style,
        };
        spans.push(Span::styled(span.content, style));
    }
    Line::from(spans)
}

fn diff_lines(path: &str, old: &str, new: &str) -> Vec<Line<'static>> {
    let diff = TextDiff::from_lines(old, new);

    // Highlight each side as a whole so multi-line constructs (strings, block
    // comments) are coloured correctly, then index back per changed line.
    let old_lines = highlight(old, path);
    let new_lines = highlight(new, path);

    let mut out = Vec::new();
    for change in diff.iter_all_changes() {
        // Inserted rows index into the new side; kept and deleted rows index
        // into the old side.
        let (marker, background, source, index) = match change.tag() {
            ChangeTag::Delete => ("-", removed_bg(), &old_lines, change.old_index()),
            ChangeTag::Insert => ("+", added_bg(), &new_lines, change.new_index()),
            ChangeTag::Equal => (" ", Style::default(), &old_lines, change.old_index()),
        };
        let content = index
            .and_then(|i| source.get(i).cloned())
            .unwrap_or_default();
        out.push(gutter_line(marker, background, content));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn edit_file_is_classified_as_a_diff() {
        let args = r#"{"path":"/a/b.rs","old_string":"let x = 1;","new_string":"let x = 2;"}"#;
        let view = tool_view("edit_file", args).expect("should classify");
        assert!(matches!(view, ToolView::Diff { path, .. } if path == "/a/b.rs"));
    }

    #[test]
    fn incomplete_streaming_arguments_are_not_classified() {
        // Mid-stream the args are not yet valid JSON; showing a broken diff
        // would be worse than showing nothing.
        assert!(tool_view("edit_file", r#"{"path":"/a/b.rs","old_st"#).is_none());
        assert!(tool_view("edit_file", "").is_none());
    }

    #[test]
    fn tools_without_a_file_view_are_skipped() {
        assert!(tool_view("shell", r#"{"command":"ls"}"#).is_none());
        assert!(tool_view("grep", r#"{"pattern":"x","path":"/a"}"#).is_none());
    }

    #[test]
    fn edit_missing_a_field_is_not_classified() {
        assert!(tool_view("edit_file", r#"{"path":"/a/b.rs"}"#).is_none());
    }

    #[test]
    fn summary_counts_added_and_removed_lines() {
        let view = ToolView::Diff {
            path: "x.rs".into(),
            old: "a\nb\nc\n".into(),
            new: "a\nB\nc\nd\n".into(),
        };
        assert_eq!(summary(&view, None), "x.rs (+2 −1)");
    }

    #[test]
    fn diff_marks_changed_lines() {
        let view = ToolView::Diff {
            path: "x.rs".into(),
            old: "keep\nold\n".into(),
            new: "keep\nnew\n".into(),
        };
        let rendered = plain(&body(&view, None, 100));
        assert_eq!(rendered, vec!["  keep", "- old", "+ new"]);
    }

    #[test]
    fn write_shows_every_line_as_added() {
        let view = ToolView::Write {
            path: "x.rs".into(),
            content: "one\ntwo\n".into(),
        };
        assert_eq!(plain(&body(&view, None, 100)), vec!["+ one", "+ two"]);
    }

    #[test]
    fn read_uses_the_tool_result_for_content() {
        let view = ToolView::Read {
            path: "x.rs".into(),
        };
        assert_eq!(plain(&body(&view, Some("a\nb\n"), 100)), vec!["  a", "  b"]);
        // Before the result arrives there is nothing to show.
        assert!(body(&view, None, 100).is_empty());
    }

    #[test]
    fn long_output_is_truncated_with_a_count() {
        let content = (0..50).map(|i| format!("line {i}")).collect::<Vec<_>>();
        let view = ToolView::Write {
            path: "x.rs".into(),
            content: content.join("\n"),
        };
        let rendered = plain(&body(&view, None, 10));
        assert_eq!(rendered.len(), 11, "10 lines plus the count");
        assert_eq!(rendered[10], "  … 40 more lines");
    }

    #[test]
    fn syntax_highlighting_colors_rust_keywords() {
        let view = ToolView::Write {
            path: "x.rs".into(),
            content: "fn main() {}".into(),
        };
        let lines = body(&view, None, 10);
        let keyword = lines[0]
            .spans
            .iter()
            .find(|s| s.content.trim() == "fn")
            .expect("`fn` should be its own span");
        assert!(keyword.style.fg.is_some(), "keyword should be coloured");
    }

    #[test]
    fn unknown_extensions_still_render() {
        let view = ToolView::Write {
            path: "notes.zzz".into(),
            content: "hello".into(),
        };
        assert_eq!(plain(&body(&view, None, 10)), vec!["+ hello"]);
    }

    #[test]
    fn added_and_removed_rows_carry_backgrounds() {
        let view = ToolView::Diff {
            path: "x.rs".into(),
            old: "old\n".into(),
            new: "new\n".into(),
        };
        let lines = body(&view, None, 100);
        for line in &lines {
            for span in &line.spans {
                assert!(span.style.bg.is_some(), "diff rows should be tinted");
            }
        }
    }
}
