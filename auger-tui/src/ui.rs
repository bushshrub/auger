use crate::app::App;
use crate::app::MsgLayout;
use crate::app::View;
use crate::types::ChatItem;
use crate::types::Status;
use crate::types::ToolDecision;
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Block;
use ratatui::widgets::Clear;
use ratatui::widgets::Borders;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

// `&mut App` because the chat pane records its row → item map as it renders;
// that mapping is what makes clicks hit-testable.
pub fn render(frame: &mut Frame, app: &mut App) {
    match app.view {
        View::SessionList => render_session_list(frame, app),
        View::Chat => render_chat(frame, app),
    }
}

fn render_session_list(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    // Header
    let header = Paragraph::new(Line::from(vec![
        Span::styled("auger", Style::default().bold()),
        Span::raw("  Sessions"),
    ]));
    frame.render_widget(header, header_area);

    // Session list
    if app.sessions.is_empty() {
        let msg = Paragraph::new("No sessions. Press [n] to create one.")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(msg, body_area);
    } else {
        let items: Vec<ListItem> = app
            .sessions
            .iter()
            .map(|s| {
                let id_short = s.session_id.to_string()[..8].to_string();
                ListItem::new(Line::from(vec![
                    Span::styled(id_short, Style::default().fg(Color::Cyan)),
                    Span::raw("  "),
                    Span::styled(&s.model, Style::default().fg(Color::White)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Sessions"))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        let mut state = app.session_list_state.clone();
        frame.render_stateful_widget(list, body_area, &mut state);
    }

    // Footer
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("[n]", Style::default().fg(Color::Yellow)),
        Span::raw(" new  "),
        Span::styled("[Enter]", Style::default().fg(Color::Yellow)),
        Span::raw(" open  "),
        Span::styled("[q]", Style::default().fg(Color::Yellow)),
        Span::raw(" quit"),
    ]));
    frame.render_widget(footer, footer_area);
}

fn render_chat(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [header_area, ctx_area, body_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .areas(area);

    render_chat_header(frame, app, header_area);
    render_ctx_bar(frame, app, ctx_area);
    render_messages(frame, app, body_area);
    render_input(frame, app, input_area);
    // Drawn last so it sits over the transcript, like an editor's popup.
    render_completion(frame, app, body_area);
}

/// Slash-command popup, anchored to the bottom of the message pane so it
/// appears directly above the input line.
fn render_completion(frame: &mut Frame, app: &App, body_area: Rect) {
    let matches = app.completion.matches(&app.input);
    if matches.is_empty() || body_area.height < 3 {
        return;
    }

    let width = matches
        .iter()
        .map(|m| m.name.width() + m.description.width() + 5)
        .max()
        .unwrap_or(20)
        .min(body_area.width as usize) as u16;
    let height = (matches.len() as u16 + 2).min(body_area.height);
    let area = Rect {
        x: body_area.x,
        y: body_area.y + body_area.height - height,
        width,
        height,
    };

    let selected = app.completion.selected(&app.input);
    let rows: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let name_style = Style::default().fg(Color::Cyan).add_modifier(
                if i == selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                },
            );
            let row = Line::from(vec![
                Span::styled(m.name, name_style),
                Span::raw("  "),
                Span::styled(m.description, Style::default().fg(Color::DarkGray)),
            ]);
            match i == selected {
                true => ListItem::new(row).style(Style::default().bg(Color::DarkGray)),
                false => ListItem::new(row),
            }
        })
        .collect();

    frame.render_widget(Clear, area);
    frame.render_widget(
        List::new(rows).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Commands  [Tab] complete  [Enter] run"),
        ),
        area,
    );
}

fn render_chat_header(frame: &mut Frame, app: &App, area: Rect) {
    let session_id_short = app
        .session_id
        .map(|id| id.to_string()[..8].to_string())
        .unwrap_or_else(|| "--------".to_string());

    let (status_text, status_style) = match app.status {
        Status::Idle => ("idle", Style::default().fg(Color::Green)),
        Status::Running => ("running", Style::default().fg(Color::Yellow)),
        Status::Connecting => ("connecting", Style::default().fg(Color::Blue)),
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled("auger", Style::default().bold()),
        Span::raw("  "),
        Span::styled(session_id_short, Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(status_text, status_style),
        Span::raw("  "),
        // Token counter, ticking while the model streams.
        Span::styled(
            format!(
                "in {}  out {}",
                fmt_tokens(app.tokens_in),
                fmt_tokens(app.tokens_out_live())
            ),
            Style::default().fg(Color::Magenta),
        ),
        Span::raw("  "),
        Span::styled(
            "[Esc] sessions  [q] quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(header, area);
}

/// Compact token count: 812, 1.2k, 45.6k.
fn fmt_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

fn render_ctx_bar(frame: &mut Frame, app: &App, area: Rect) {
    if app.ctx_window == 0 {
        return;
    }
    let pct = (app.ctx_used as f64 / app.ctx_window as f64 * 100.0).min(100.0);
    let bar_width = (area.width as usize).saturating_sub(20);
    let filled = ((pct / 100.0) * bar_width as f64) as usize;
    let empty = bar_width.saturating_sub(filled);

    let bar_color = if pct >= 90.0 {
        Color::Red
    } else if pct >= 75.0 {
        Color::Yellow
    } else {
        Color::Blue
    };

    let bar: String = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
    let label = format!(
        " {:.0}%  {}k/{}k tok",
        pct,
        app.ctx_used / 1000,
        app.ctx_window / 1000
    );

    let line = Paragraph::new(Line::from(vec![
        Span::styled(bar, Style::default().fg(bar_color)),
        Span::styled(label, Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(line, area);
}

fn render_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = vec![];
    // Parallel to `lines`: which chat item produced each row.
    let mut row_items: Vec<Option<usize>> = vec![];

    for (idx, item) in app.items.iter().enumerate() {
        // Spacer row between items belongs to no item, so clicking the gap
        // does nothing.
        lines.push(Line::from(""));
        row_items.push(None);

        match item {
            ChatItem::User { text } => {
                for l in wrap_text(text, width) {
                    lines.push(Line::from(Span::styled(
                        format!("  > {l}"),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    )));
                }
            }

            ChatItem::Assistant { text } => {
                // Assistant output is markdown; render it styled and
                // pre-wrapped, then indent to match the other item kinds.
                for line in crate::markdown::render_markdown(text, width.saturating_sub(2)) {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(line.spans);
                    lines.push(Line::from(spans));
                }
            }

            ChatItem::Reasoning { text, collapsed } => {
                if *collapsed {
                    lines.push(Line::from(Span::styled(
                        format!("  ▸ thinking ({} chars — click to expand)", text.len()),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        "  ▾ thinking (click to collapse)",
                        Style::default().fg(Color::DarkGray),
                    )));
                    for l in wrap_text(text, width.saturating_sub(4)) {
                        lines.push(Line::from(Span::styled(
                            format!("    {l}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }

            ChatItem::Tool {
                id,
                name,
                args,
                result,
                decision,
                expanded,
            } => {
                let (decision_str, decision_style) = match decision {
                    Some(ToolDecision::Approved) => {
                        (" [approved]", Style::default().fg(Color::Green))
                    }
                    Some(ToolDecision::Denied) => (" [denied]", Style::default().fg(Color::Red)),
                    Some(ToolDecision::Auto) => (" [auto]", Style::default().fg(Color::DarkGray)),
                    None => ("", Style::default()),
                };

                lines.push(Line::from(vec![
                    Span::styled("  tool ", Style::default().fg(Color::Magenta)),
                    Span::styled(
                        name,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(decision_str, decision_style),
                ]));

                // File-touching tools get a diff or preview; everything else
                // keeps the plain argument summary.
                match crate::diff::tool_view(name, args) {
                    Some(view) => {
                        let marker = if *expanded { "▾" } else { "▸" };
                        let hint = if *expanded {
                            " — click to collapse"
                        } else {
                            " — click to expand"
                        };
                        lines.push(Line::from(Span::styled(
                            format!(
                                "    {marker} {}{hint}",
                                crate::diff::summary(&view, result.as_deref())
                            ),
                            Style::default().fg(Color::DarkGray),
                        )));

                        if *expanded {
                            // Cap the body at the pane height so one huge file
                            // can't bury the rest of the conversation.
                            let cap = area.height.saturating_sub(2).max(1) as usize;
                            for line in crate::diff::body(&view, result.as_deref(), cap) {
                                let mut spans = vec![Span::raw("    ")];
                                spans.extend(line.spans);
                                lines.push(Line::from(spans));
                            }
                        }
                    }
                    // A call still awaiting a decision always shows its
                    // arguments in full: that is what is being approved.
                    None => {
                        let awaiting = decision.is_none() && app.pending_tools.iter().any(|t| t == id);
                        let cap = area.height.saturating_sub(2).max(1) as usize;
                        for line in crate::toolargs::render(
                            name,
                            args,
                            result.as_deref(),
                            *expanded || awaiting,
                            width.saturating_sub(4),
                            cap,
                        ) {
                            let mut spans = vec![Span::raw("    ")];
                            spans.extend(line.spans);
                            lines.push(Line::from(spans));
                        }
                    }
                }

                if decision.is_none() && app.pending_tool() == Some(id.as_str()) {
                    let queued = app.pending_tools.len();
                    let mut prompt = vec![
                        Span::raw("  "),
                        Span::styled("[y] approve", Style::default().fg(Color::Green)),
                        Span::raw("  "),
                        Span::styled("[n] deny", Style::default().fg(Color::Red)),
                    ];
                    if queued > 1 {
                        prompt.push(Span::raw("  "));
                        prompt.push(Span::styled(
                            format!("[a] approve all {queued}"),
                            Style::default().fg(Color::Green),
                        ));
                        prompt.push(Span::raw("  "));
                        prompt.push(Span::styled(
                            format!("[d] deny all {queued}"),
                            Style::default().fg(Color::Red),
                        ));
                    }
                    lines.push(Line::from(prompt));
                } else if decision.is_none() && app.pending_tools.iter().any(|t| t == id) {
                    lines.push(Line::from(Span::styled(
                        "    waiting for the calls above",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }

            ChatItem::Error { text } => {
                lines.push(Line::from(Span::styled(
                    format!("  ⚠ {text}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }

        // Every row this item produced maps back to it.
        row_items.resize(lines.len(), Some(idx));
    }

    if lines.is_empty() {
        app.msg_layout = MsgLayout::default();
        let placeholder = Paragraph::new("No messages yet. Type below and press Enter to send.")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(placeholder, area);
        return;
    }

    // We pre-wrap every line ourselves so Paragraph doesn't need to rewrap.
    // This makes lines.len() an accurate count of visual rows for scroll math.
    let total_lines = lines.len() as u16;
    let visible = area.height.saturating_sub(2); // subtract top/bottom border
    let max_scroll = total_lines.saturating_sub(visible);
    // scroll_from_bottom=0 → pinned to bottom; larger → further up
    let scroll_top = max_scroll.saturating_sub(app.scroll_from_bottom);

    app.msg_layout = MsgLayout {
        area,
        scroll_top,
        row_items,
    };

    let text = Text::from(lines);
    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL))
        .scroll((scroll_top, 0));
    frame.render_widget(para, area);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let pending = app.pending_tools.len();
    // The approval prompt outranks the busy notice: the run is blocked on it.
    let (placeholder, style) = if pending > 0 {
        (
            if pending > 1 {
                format!("{pending} tool calls: [y]/[n] one, [a]/[d] all")
            } else {
                "press [y] approve / [n] deny".to_string()
            },
            Style::default().fg(Color::Yellow),
        )
    } else if app.status != Status::Idle {
        (
            "agent is busy...".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    } else {
        (
            "Message... (Enter to send, /help for commands)".to_string(),
            Style::default(),
        )
    };

    let display = if app.input.is_empty() {
        Span::styled(placeholder, Style::default().fg(Color::DarkGray))
    } else {
        Span::raw(app.input.as_str())
    };

    let input = Paragraph::new(Line::from(display))
        .style(style)
        .block(Block::default().borders(Borders::ALL).title("Input"));
    frame.render_widget(input, area);

    // Place the cursor at the edit position, measured in display columns —
    // byte offsets and columns diverge for non-ASCII input.
    if app.status == Status::Idle && !app.has_pending_tool() {
        let cursor_x = area.x + 1 + app.cursor_column() as u16;
        let cursor_y = area.y + 1;
        if cursor_x < area.x + area.width - 1 {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

pub(crate) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = vec![];
    for raw_line in text.lines() {
        if raw_line.len() <= width {
            lines.push(raw_line.to_string());
        } else {
            let mut remaining = raw_line;
            while !remaining.is_empty() {
                let split = remaining
                    .char_indices()
                    .take_while(|(i, _)| *i < width)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(width.min(remaining.len()));
                lines.push(remaining[..split].to_string());
                remaining = &remaining[split..];
            }
        }
    }
    lines
}
