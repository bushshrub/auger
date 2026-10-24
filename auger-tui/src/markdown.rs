//! Markdown → ratatui renderer.
//!
//! Parses markdown with comrak and emits pre-wrapped [`Line`]s built from
//! styled [`Span`]s. Styling is carried by ratatui [`Style`] values, never by
//! ANSI escape codes — ratatui does not interpret escapes inside a `Line`, so
//! they would render as literal garbage.
//!
//! Every line comes back already wrapped to `width`, which lets the caller
//! treat `lines.len()` as an exact visual row count for scroll math.

use comrak::Arena;
use comrak::Options;
use comrak::nodes::AstNode;
use comrak::nodes::ListDelimType;
use comrak::nodes::ListType;
use comrak::nodes::NodeValue;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// Render markdown into wrapped, styled lines no wider than `width` columns.
pub fn render_markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    if width == 0 || text.trim().is_empty() {
        return vec![];
    }

    let arena = Arena::new();
    let root = comrak::parse_document(&arena, text, &options());

    let mut out = Vec::new();
    render_children(root, &mut out, &[], width);
    trim_blank_edges(&mut out);
    out
}

fn options<'c>() -> Options<'c> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    options.extension.footnotes = true;
    options
}

// ── styles ───────────────────────────────────────────────────────────────────

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn code_style() -> Style {
    Style::default().fg(Color::Cyan)
}

fn heading_style(level: u8) -> Style {
    let base = Style::default().add_modifier(Modifier::BOLD);
    match level {
        1 => base.fg(Color::Magenta),
        2 => base.fg(Color::Cyan),
        _ => base.fg(Color::Blue),
    }
}

// ── inline segments ──────────────────────────────────────────────────────────

/// A styled run of text small enough to be a wrapping unit. Whitespace runs are
/// tracked separately so they can be dropped at a line break.
#[derive(Clone)]
struct Seg {
    text: String,
    style: Style,
    is_space: bool,
}

impl Seg {
    fn width(&self) -> usize {
        self.text.width()
    }
}

/// Inline content of one block, split into hard-broken lines. A markdown hard
/// break (`\` or two trailing spaces) starts a new inner vector.
#[derive(Default)]
struct Inlines {
    lines: Vec<Vec<Seg>>,
}

impl Inlines {
    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        let current = self.lines.last_mut().expect("just ensured non-empty");
        // Split into alternating word / whitespace runs so the wrapper can
        // break on whitespace and discard it at the break.
        for (is_space, group) in group_by_space(text) {
            current.push(Seg {
                text: group,
                style,
                is_space,
            });
        }
    }

    fn hard_break(&mut self) {
        self.lines.push(Vec::new());
    }

    fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.iter().all(|s| s.is_space))
    }
}

fn group_by_space(text: &str) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, String)> = Vec::new();
    for ch in text.chars() {
        let is_space = ch.is_whitespace();
        match out.last_mut() {
            Some((prev, buf)) if *prev == is_space => buf.push(ch),
            _ => out.push((is_space, ch.to_string())),
        }
    }
    out
}

/// Walk inline nodes, accumulating styled segments.
fn collect_inlines<'a>(node: &'a AstNode<'a>, out: &mut Inlines, style: Style) {
    for child in node.children() {
        let value = &child.data.borrow().value;
        match value {
            NodeValue::Text(text) => out.push_text(text, style),

            NodeValue::Code(code) => out.push_text(&code.literal, code_style()),

            NodeValue::Emph => {
                collect_inlines(child, out, style.add_modifier(Modifier::ITALIC));
            }
            NodeValue::Strong => {
                collect_inlines(child, out, style.add_modifier(Modifier::BOLD));
            }
            NodeValue::Strikethrough => {
                collect_inlines(child, out, style.add_modifier(Modifier::CROSSED_OUT));
            }
            NodeValue::Underline => {
                collect_inlines(child, out, style.add_modifier(Modifier::UNDERLINED));
            }
            NodeValue::Superscript | NodeValue::Subscript => {
                collect_inlines(child, out, style);
            }
            NodeValue::SpoileredText => {
                collect_inlines(child, out, style.add_modifier(Modifier::DIM));
            }

            NodeValue::Link(link) => {
                let link_style = style.fg(Color::Blue).add_modifier(Modifier::UNDERLINED);
                collect_inlines(child, out, link_style);
                // Autolinks already show the URL as their text; don't repeat it.
                if !link.url.is_empty() && !link_text_is_url(child, &link.url) {
                    out.push_text(" (", dim());
                    out.push_text(&link.url, dim());
                    out.push_text(")", dim());
                }
            }

            NodeValue::Image(link) => {
                out.push_text("[image: ", dim());
                collect_inlines(child, out, dim());
                if !link.url.is_empty() {
                    out.push_text(" ", dim());
                    out.push_text(&link.url, dim());
                }
                out.push_text("]", dim());
            }

            NodeValue::Math(math) => out.push_text(&math.literal, code_style()),

            NodeValue::FootnoteReference(footnote) => {
                out.push_text(&format!("[{}]", footnote.name), dim());
            }

            // Soft breaks are paragraph-internal newlines; the wrapper decides
            // where lines actually break, so they become spaces.
            NodeValue::SoftBreak => out.push_text(" ", style),
            NodeValue::LineBreak => out.hard_break(),

            // Raw HTML has no terminal representation; drop the tags.
            NodeValue::HtmlInline(_) | NodeValue::EscapedTag(_) => {}

            NodeValue::Raw(text) => out.push_text(text, style),
            NodeValue::Escaped => collect_inlines(child, out, style),

            // Anything else: descend so nested text is never lost.
            _ => collect_inlines(child, out, style),
        }
    }
}

/// True when a link's visible text is just its URL (comrak's autolink output),
/// in which case appending the URL again would duplicate it.
fn link_text_is_url<'a>(node: &'a AstNode<'a>, url: &str) -> bool {
    let mut text = String::new();
    for descendant in node.descendants() {
        if let NodeValue::Text(t) = &descendant.data.borrow().value {
            text.push_str(t);
        }
    }
    let text = text.trim();
    text == url || url == format!("mailto:{text}")
}

// ── wrapping ─────────────────────────────────────────────────────────────────

/// Greedily wrap segments to `width`, prefixing the first output line with
/// `first_prefix` and the rest with `cont_prefix`.
fn wrap_segs(
    segs: &[Seg],
    width: usize,
    first_prefix: &[Span<'static>],
    cont_prefix: &[Span<'static>],
) -> Vec<Line<'static>> {
    let first_w = spans_width(first_prefix);
    let cont_w = spans_width(cont_prefix);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Seg> = Vec::new();
    let mut used = 0usize;
    let mut avail = width.saturating_sub(first_w).max(1);

    let flush = |current: &mut Vec<Seg>, lines: &mut Vec<Line<'static>>| {
        // Trailing whitespace would otherwise pad the line to no effect.
        while current.last().is_some_and(|s| s.is_space) {
            current.pop();
        }
        let prefix = if lines.is_empty() {
            first_prefix
        } else {
            cont_prefix
        };
        let mut spans: Vec<Span<'static>> = prefix.to_vec();
        spans.extend(current.drain(..).map(|s| Span::styled(s.text, s.style)));
        lines.push(Line::from(spans));
    };

    for seg in segs {
        // Leading whitespace on a fresh line is dropped.
        if seg.is_space && current.is_empty() {
            continue;
        }

        let seg_w = seg.width();
        if used + seg_w <= avail {
            used += seg_w;
            current.push(seg.clone());
            continue;
        }

        // Whitespace that doesn't fit just becomes the break itself.
        if seg.is_space {
            flush(&mut current, &mut lines);
            used = 0;
            avail = width.saturating_sub(cont_w).max(1);
            continue;
        }

        // A word that fits on a line of its own: break before it.
        if seg_w <= avail || !current.is_empty() {
            flush(&mut current, &mut lines);
            used = 0;
            avail = width.saturating_sub(cont_w).max(1);
        }

        if seg.width() <= avail {
            used = seg.width();
            current.push(seg.clone());
            continue;
        }

        // A single word longer than the line: hard-split it across lines.
        for piece in split_to_width(&seg.text, avail) {
            let piece_w = piece.width();
            if used + piece_w > avail && !current.is_empty() {
                flush(&mut current, &mut lines);
                used = 0;
                avail = width.saturating_sub(cont_w).max(1);
            }
            used += piece_w;
            current.push(Seg {
                text: piece,
                style: seg.style,
                is_space: false,
            });
        }
    }

    if !current.is_empty() || lines.is_empty() {
        flush(&mut current, &mut lines);
    }

    lines
}

/// Split a string into chunks of at most `width` display columns, never
/// splitting inside a character.
fn split_to_width(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;

    for ch in text.chars() {
        let ch_w = ch.width().unwrap_or(0);
        if used + ch_w > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += ch_w;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

// ── block rendering ──────────────────────────────────────────────────────────

fn render_children<'a>(
    node: &'a AstNode<'a>,
    out: &mut Vec<Line<'static>>,
    prefix: &[Span<'static>],
    width: usize,
) {
    for child in node.children() {
        render_block(child, out, prefix, width);
    }
}

/// Append a blank line as a block separator, collapsing runs of blanks.
fn separate(out: &mut Vec<Line<'static>>, prefix: &[Span<'static>]) {
    if out.is_empty() {
        return;
    }
    if out.last().is_some_and(is_blank_line) {
        return;
    }
    out.push(Line::from(prefix.to_vec()));
}

fn is_blank_line(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

fn trim_blank_edges(lines: &mut Vec<Line<'static>>) {
    while lines.first().is_some_and(is_blank_line) {
        lines.remove(0);
    }
    while lines.last().is_some_and(is_blank_line) {
        lines.pop();
    }
}

fn render_block<'a>(
    node: &'a AstNode<'a>,
    out: &mut Vec<Line<'static>>,
    prefix: &[Span<'static>],
    width: usize,
) {
    let value = node.data.borrow().value.clone();

    match value {
        NodeValue::Document | NodeValue::FrontMatter(_) => {
            render_children(node, out, prefix, width);
        }

        NodeValue::Paragraph => {
            separate(out, prefix);
            render_inline_block(node, out, prefix, prefix, width, Style::default());
        }

        NodeValue::Heading(heading) => {
            separate(out, prefix);
            let marker = Span::styled("#".repeat(heading.level as usize) + " ", dim());
            let mut first: Vec<Span<'static>> = prefix.to_vec();
            first.push(marker);
            render_inline_block(
                node,
                out,
                &first,
                prefix,
                width,
                heading_style(heading.level),
            );
        }

        NodeValue::CodeBlock(block) => {
            separate(out, prefix);
            render_code_block(&block.info, &block.literal, out, prefix, width);
        }

        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            separate(out, prefix);
            let mut inner: Vec<Span<'static>> = prefix.to_vec();
            inner.push(Span::styled("│ ", dim()));
            render_children(node, out, &inner, width);
        }

        NodeValue::Alert(alert) => {
            separate(out, prefix);
            let mut inner: Vec<Span<'static>> = prefix.to_vec();
            inner.push(Span::styled("│ ", dim()));
            let title = alert
                .title
                .clone()
                .unwrap_or_else(|| format!("{:?}", alert.alert_type).to_uppercase());
            let mut header = inner.clone();
            header.push(Span::styled(
                title,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            out.push(Line::from(header));
            render_children(node, out, &inner, width);
        }

        NodeValue::List(_) => {
            // A list nested directly inside an item continues that item; a
            // blank line there would break the list apart visually.
            let nested = node
                .parent()
                .is_some_and(|p| matches!(p.data.borrow().value, NodeValue::Item(_)));
            if !nested {
                separate(out, prefix);
            }
            render_children(node, out, prefix, width);
        }

        NodeValue::Item(list) => {
            render_item(node, &list, None, out, prefix, width);
        }

        NodeValue::TaskItem(symbol) => {
            let list = match node.parent().map(|p| p.data.borrow().value.clone()) {
                Some(NodeValue::List(list)) => list,
                _ => Default::default(),
            };
            render_item(node, &list, Some(symbol), out, prefix, width);
        }

        NodeValue::ThematicBreak => {
            separate(out, prefix);
            let rule_width = width.saturating_sub(spans_width(prefix)).max(1);
            let mut spans: Vec<Span<'static>> = prefix.to_vec();
            spans.push(Span::styled("─".repeat(rule_width), dim()));
            out.push(Line::from(spans));
        }

        NodeValue::Table(_) => {
            separate(out, prefix);
            render_table(node, out, prefix, width);
        }

        NodeValue::FootnoteDefinition(footnote) => {
            separate(out, prefix);
            let mut inner: Vec<Span<'static>> = prefix.to_vec();
            inner.push(Span::styled(format!("[{}] ", footnote.name), dim()));
            render_children(node, out, &inner, width);
        }

        NodeValue::HtmlBlock(_) => {
            // Raw HTML has no meaningful terminal rendering; drop it.
        }

        NodeValue::DescriptionList | NodeValue::DescriptionItem(_) => {
            render_children(node, out, prefix, width);
        }
        NodeValue::DescriptionTerm => {
            separate(out, prefix);
            render_inline_block(
                node,
                out,
                prefix,
                prefix,
                width,
                Style::default().add_modifier(Modifier::BOLD),
            );
        }
        NodeValue::DescriptionDetails => {
            let mut inner: Vec<Span<'static>> = prefix.to_vec();
            inner.push(Span::raw("  "));
            render_children(node, out, &inner, width);
        }

        // Inline nodes reached at block position (e.g. a table cell's contents
        // handled elsewhere): render them as a standalone wrapped line.
        _ => {
            render_inline_block(node, out, prefix, prefix, width, Style::default());
        }
    }
}

/// Collect a node's inline children and emit them as wrapped lines.
fn render_inline_block<'a>(
    node: &'a AstNode<'a>,
    out: &mut Vec<Line<'static>>,
    first_prefix: &[Span<'static>],
    cont_prefix: &[Span<'static>],
    width: usize,
    style: Style,
) {
    let mut inlines = Inlines::default();
    collect_inlines(node, &mut inlines, style);
    if inlines.is_empty() {
        return;
    }

    let mut first = true;
    for segs in &inlines.lines {
        let prefix = if first { first_prefix } else { cont_prefix };
        out.extend(wrap_segs(segs, width, prefix, cont_prefix));
        first = false;
    }
}

fn render_item<'a>(
    node: &'a AstNode<'a>,
    list: &comrak::nodes::NodeList,
    task: Option<Option<char>>,
    out: &mut Vec<Line<'static>>,
    prefix: &[Span<'static>],
    width: usize,
) {
    let marker = match list.list_type {
        ListType::Ordered => {
            // Each item carries its own `NodeList` whose `start` is that item's
            // ordinal, already accounting for the list's start value.
            let index = list.start;
            let delim = match list.delimiter {
                ListDelimType::Period => '.',
                ListDelimType::Paren => ')',
            };
            format!("{index}{delim} ")
        }
        ListType::Bullet => "• ".to_string(),
    };

    let mut first: Vec<Span<'static>> = prefix.to_vec();
    first.push(Span::styled(
        marker.clone(),
        Style::default().fg(Color::Yellow),
    ));

    if let Some(symbol) = task {
        let (glyph, style) = match symbol {
            Some(_) => ("[x] ", Style::default().fg(Color::Green)),
            None => ("[ ] ", dim()),
        };
        first.push(Span::styled(glyph, style));
    }

    // Continuation lines and nested blocks align under the item text.
    let mut cont: Vec<Span<'static>> = prefix.to_vec();
    cont.push(Span::raw(" ".repeat(marker.width())));

    // The first child renders against the marker prefix; later children (nested
    // lists, extra paragraphs) align with the continuation indent.
    let mut children = node.children();
    if let Some(child) = children.next() {
        let value = child.data.borrow().value.clone();
        if matches!(value, NodeValue::Paragraph) {
            render_inline_block(child, out, &first, &cont, width, Style::default());
        } else {
            render_block(child, out, &first, width);
        }
    } else {
        out.push(Line::from(first));
    }

    for child in children {
        render_block(child, out, &cont, width);
    }
}

fn render_code_block(
    info: &str,
    literal: &str,
    out: &mut Vec<Line<'static>>,
    prefix: &[Span<'static>],
    width: usize,
) {
    let lang = info.split_whitespace().next().unwrap_or("");
    let avail = width
        .saturating_sub(spans_width(prefix))
        .saturating_sub(2)
        .max(1);

    let mut header: Vec<Span<'static>> = prefix.to_vec();
    header.push(Span::styled(
        if lang.is_empty() {
            "┌─ code".to_string()
        } else {
            format!("┌─ {lang}")
        },
        dim(),
    ));
    out.push(Line::from(header));

    for raw in literal.lines() {
        // Tabs would render at an unpredictable width inside a bordered block.
        let expanded = raw.replace('\t', "    ");
        for piece in split_to_width(&expanded, avail) {
            let mut spans: Vec<Span<'static>> = prefix.to_vec();
            spans.push(Span::styled("│ ", dim()));
            spans.push(Span::styled(piece, Style::default().fg(Color::Green)));
            out.push(Line::from(spans));
        }
    }

    let mut footer: Vec<Span<'static>> = prefix.to_vec();
    footer.push(Span::styled("└─", dim()));
    out.push(Line::from(footer));
}

// ── tables ───────────────────────────────────────────────────────────────────

fn render_table<'a>(
    node: &'a AstNode<'a>,
    out: &mut Vec<Line<'static>>,
    prefix: &[Span<'static>],
    width: usize,
) {
    // Flatten to plain text: a wrapped, styled table is more trouble than it's
    // worth in a chat pane, but column alignment still carries the structure.
    let mut rows: Vec<(bool, Vec<String>)> = Vec::new();
    for row in node.descendants() {
        let is_header = match &row.data.borrow().value {
            NodeValue::TableRow(is_header) => *is_header,
            _ => continue,
        };
        let cells = row
            .children()
            .map(|cell| {
                let mut inlines = Inlines::default();
                collect_inlines(cell, &mut inlines, Style::default());
                inlines
                    .lines
                    .iter()
                    .map(|segs| segs.iter().map(|s| s.text.as_str()).collect::<String>())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .trim()
                    .to_string()
            })
            .collect::<Vec<_>>();
        rows.push((is_header, cells));
    }

    if rows.is_empty() {
        return;
    }

    let columns = rows.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for (_, cells) in &rows {
        for (i, cell) in cells.iter().enumerate() {
            widths[i] = widths[i].max(cell.width());
        }
    }

    // Shrink the widest columns until the table fits the pane.
    let avail = width.saturating_sub(spans_width(prefix));
    let separators = columns.saturating_sub(1) * 3;
    while widths.iter().sum::<usize>() + separators > avail {
        let widest = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .map(|(i, _)| i);
        match widest {
            Some(i) if widths[i] > 1 => widths[i] -= 1,
            _ => break,
        }
    }

    for (is_header, cells) in &rows {
        let style = if *is_header {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let mut spans: Vec<Span<'static>> = prefix.to_vec();
        for (i, target) in widths.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", dim()));
            }
            let cell = cells.get(i).map(String::as_str).unwrap_or("");
            spans.push(Span::styled(pad_to(cell, *target), style));
        }
        out.push(Line::from(spans));

        if *is_header {
            let mut rule: Vec<Span<'static>> = prefix.to_vec();
            for (i, target) in widths.iter().enumerate() {
                if i > 0 {
                    rule.push(Span::styled("─┼─", dim()));
                }
                rule.push(Span::styled("─".repeat(*target), dim()));
            }
            out.push(Line::from(rule));
        }
    }
}

/// Pad or truncate to exactly `target` display columns.
fn pad_to(text: &str, target: usize) -> String {
    let current = text.width();
    if current == target {
        return text.to_string();
    }
    if current < target {
        return format!("{text}{}", " ".repeat(target - current));
    }
    let mut out = String::new();
    let mut used = 0usize;
    let budget = target.saturating_sub(1);
    for ch in text.chars() {
        let ch_w = ch.width().unwrap_or(0);
        if used + ch_w > budget {
            break;
        }
        out.push(ch);
        used += ch_w;
    }
    out.push('…');
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
    fn empty_input_renders_nothing() {
        assert!(render_markdown("", 40).is_empty());
        assert!(render_markdown("   \n  ", 40).is_empty());
        assert!(render_markdown("hello", 0).is_empty());
    }

    #[test]
    fn paragraph_wraps_at_width() {
        let lines = render_markdown("aaa bbb ccc ddd eee", 11);
        assert_eq!(plain(&lines), vec!["aaa bbb ccc", "ddd eee"]);
    }

    #[test]
    fn every_line_fits_the_width() {
        let text = "Some **bold** text with `code` and a very long word \
                    supercalifragilisticexpialidocious plus more words here.";
        for width in [8usize, 20, 40] {
            for line in render_markdown(text, width) {
                let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(
                    rendered.width() <= width,
                    "width {width}: {rendered:?} is {} cols",
                    rendered.width()
                );
            }
        }
    }

    #[test]
    fn inline_styles_apply_to_the_right_spans() {
        let lines = render_markdown("plain **bold** `code`", 40);
        let spans = &lines[0].spans;
        let bold = spans.iter().find(|s| s.content == "bold").unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = spans.iter().find(|s| s.content == "code").unwrap();
        assert_eq!(code.style.fg, Some(Color::Cyan));
        let plain = spans.iter().find(|s| s.content == "plain").unwrap();
        assert!(!plain.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn no_ansi_escapes_leak_into_output() {
        let text = "# Head\n\n*em* **strong** `c`\n\n> quote\n\n- a\n\n```rs\nfn x() {}\n```";
        for line in render_markdown(text, 40) {
            for span in &line.spans {
                assert!(!span.content.contains('\x1b'), "escape in {span:?}");
            }
        }
    }

    #[test]
    fn ordered_lists_keep_their_numbering() {
        let lines = render_markdown("1. one\n2. two\n3. three", 40);
        assert_eq!(plain(&lines), vec!["1. one", "2. two", "3. three"]);
    }

    #[test]
    fn ordered_lists_honour_start_value() {
        let lines = render_markdown("5. five\n6. six", 40);
        assert_eq!(plain(&lines), vec!["5. five", "6. six"]);
    }

    #[test]
    fn bullet_list_wraps_aligned_under_the_text() {
        let lines = render_markdown("- alpha beta gamma delta", 12);
        assert_eq!(plain(&lines), vec!["• alpha beta", "  gamma", "  delta"]);
    }

    #[test]
    fn nested_lists_indent() {
        let lines = render_markdown("- outer\n  - inner", 40);
        assert_eq!(plain(&lines), vec!["• outer", "  • inner"]);
    }

    #[test]
    fn block_quote_prefixes_each_line() {
        let lines = render_markdown("> quoted text here", 10);
        for line in &plain(&lines) {
            assert!(line.starts_with("│ "), "{line:?}");
        }
    }

    #[test]
    fn code_block_preserves_content_verbatim() {
        let lines = render_markdown("```rust\nlet x = 1;\n```", 40);
        let rendered = plain(&lines);
        assert_eq!(rendered[0], "┌─ rust");
        assert_eq!(rendered[1], "│ let x = 1;");
        assert_eq!(rendered[2], "└─");
    }

    #[test]
    fn code_block_indentation_survives() {
        let lines = render_markdown("```\nfn a() {\n    b();\n}\n```", 40);
        assert_eq!(plain(&lines)[2], "│     b();");
    }

    #[test]
    fn heading_is_styled_and_prefixed() {
        let lines = render_markdown("## Title", 40);
        assert_eq!(plain(&lines), vec!["## Title"]);
        let title = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "Title")
            .unwrap();
        assert!(title.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn nodes_render_once_not_twice() {
        let lines = render_markdown("**bold**", 40);
        assert_eq!(plain(&lines), vec!["bold"]);
    }

    #[test]
    fn soft_breaks_join_into_one_paragraph() {
        let lines = render_markdown("one\ntwo", 40);
        assert_eq!(plain(&lines), vec!["one two"]);
    }

    #[test]
    fn hard_break_splits_the_line() {
        let lines = render_markdown("one  \ntwo", 40);
        assert_eq!(plain(&lines), vec!["one", "two"]);
    }

    #[test]
    fn blocks_are_separated_by_one_blank_line() {
        let lines = render_markdown("para one\n\npara two", 40);
        assert_eq!(plain(&lines), vec!["para one", "", "para two"]);
    }

    #[test]
    fn repeated_words_wrap_correctly() {
        // The old implementation located chunks with `str::find`, which
        // misplaced them whenever text repeated.
        let lines = render_markdown("ab ab ab ab", 5);
        assert_eq!(plain(&lines), vec!["ab ab", "ab ab"]);
    }

    #[test]
    fn wide_characters_count_as_two_columns() {
        let lines = render_markdown("日本語テスト", 6);
        for line in plain(&lines) {
            assert!(line.width() <= 6, "{line:?}");
        }
        assert_eq!(plain(&lines), vec!["日本語", "テスト"]);
    }

    #[test]
    fn task_items_show_checkboxes() {
        let lines = render_markdown("- [x] done\n- [ ] todo", 40);
        assert_eq!(plain(&lines), vec!["• [x] done", "• [ ] todo"]);
    }

    #[test]
    fn tables_align_into_columns() {
        let lines = render_markdown("| a | b |\n|---|---|\n| 1 | 2 |", 40);
        assert_eq!(plain(&lines), vec!["a │ b", "──┼──", "1 │ 2"]);
    }

    #[test]
    fn link_shows_text_and_url() {
        let lines = render_markdown("[text](http://example.com)", 60);
        assert_eq!(plain(&lines), vec!["text (http://example.com)"]);
    }

    #[test]
    fn autolink_does_not_duplicate_the_url() {
        let lines = render_markdown("<http://example.com>", 60);
        assert_eq!(plain(&lines), vec!["http://example.com"]);
    }

    #[test]
    fn thematic_break_spans_the_width() {
        let lines = render_markdown("a\n\n---\n\nb", 10);
        assert_eq!(plain(&lines)[2], "─".repeat(10));
    }

    #[test]
    fn strikethrough_is_marked() {
        let lines = render_markdown("~~gone~~", 40);
        let span = lines[0].spans.iter().find(|s| s.content == "gone").unwrap();
        assert!(span.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn html_blocks_are_dropped() {
        let lines = render_markdown("<div>x</div>\n\ntext", 40);
        assert_eq!(plain(&lines), vec!["text"]);
    }

    #[test]
    fn no_leading_or_trailing_blank_lines() {
        let lines = render_markdown("\n\n# H\n\ntext\n\n\n", 40);
        assert!(!is_blank_line(lines.first().unwrap()));
        assert!(!is_blank_line(lines.last().unwrap()));
    }
}
