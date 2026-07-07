use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::app::{App, View};
use crate::types::{ChatItem, Status, ToolDecision};

pub fn render(frame: &mut Frame, app: &App) {
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

fn render_chat(frame: &mut Frame, app: &App) {
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
        Span::styled(
            "[Esc] sessions  [q] quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]));
    frame.render_widget(header, area);
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

fn render_messages(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = vec![];

    for item in &app.items {
        match item {
            ChatItem::User { text } => {
                lines.push(Line::from(""));
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
                lines.push(Line::from(""));
                for l in wrap_text(text, width) {
                    lines.push(Line::from(Span::raw(format!("  {l}"))));
                }
            }

            ChatItem::Reasoning { text, collapsed } => {
                lines.push(Line::from(""));
                if *collapsed {
                    lines.push(Line::from(Span::styled(
                        format!("  [thinking: {} chars — press r to expand]", text.len()),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        "  ── thinking ──",
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
            } => {
                lines.push(Line::from(""));
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

                if decision.is_none() && app.pending_tool_id.as_deref() == Some(id) {
                    // Show truncated args
                    let args_short = truncate_str(args, 100);
                    lines.push(Line::from(Span::styled(
                        format!("    {args_short}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled("[y] approve", Style::default().fg(Color::Green)),
                        Span::raw("  "),
                        Span::styled("[n] deny", Style::default().fg(Color::Red)),
                    ]));
                } else {
                    let args_short = truncate_str(args, 80);
                    lines.push(Line::from(Span::styled(
                        format!("    {args_short}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                    if let Some(res) = result {
                        let preview = truncate_str(res, 200);
                        lines.push(Line::from(Span::styled(
                            format!("    → {preview}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }

            ChatItem::Error { text } => {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("  ⚠ {text}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    if lines.is_empty() {
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

    let text = Text::from(lines);
    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL))
        .scroll((scroll_top, 0));
    frame.render_widget(para, area);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let (placeholder, style) = if app.status != Status::Idle {
        ("agent is busy…", Style::default().fg(Color::DarkGray))
    } else if app.pending_tool_id.is_some() {
        (
            "press [y] approve / [n] deny",
            Style::default().fg(Color::Yellow),
        )
    } else {
        ("Message… (Enter to send)", Style::default())
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

    // Place cursor after the input text
    if app.status == Status::Idle && app.pending_tool_id.is_none() {
        let cursor_x = area.x + 1 + app.input.len() as u16;
        let cursor_y = area.y + 1;
        if cursor_x < area.x + area.width - 1 {
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
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

fn truncate_str(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.len() <= max {
        s
    } else {
        format!("{}…", &s[..max])
    }
}
