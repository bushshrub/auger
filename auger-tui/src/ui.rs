use crate::app::App;
use crate::history::LiveTool;
use crate::types::Status;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Borders;
use ratatui::widgets::Clear;
use ratatui::widgets::List;
use ratatui::widgets::ListItem;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// Rows of the inline viewport at the bottom of the terminal.
pub const VIEWPORT_HEIGHT: u16 = 16;

/// Print every newly finished item above the viewport, into the terminal's
/// scrollback.
pub fn flush_finished(terminal: &mut DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    let width = terminal.size()?.width as usize;
    let lines: Vec<Line> = app
        .take_finished()
        .iter()
        .flat_map(|item| crate::history::final_lines(item, width))
        .collect();
    if lines.is_empty() {
        return Ok(());
    }
    terminal.insert_before(lines.len() as u16, |buf| {
        Paragraph::new(lines).render(buf.area, buf);
    })
}

/// Draw the live area: unfinished items, then the composer between two
/// rules, then the statusline.
pub fn render(frame: &mut Frame, app: &App) {
    // While calls wait for a decision the approval panel takes the
    // composer's place.
    let width = frame.area().width as usize;
    let panel = match app.pending_tool_call() {
        Some((name, args)) => crate::approval::lines(
            name,
            args,
            app.pending_tools.len(),
            app.approval_selected,
            width,
        ),
        None => vec![],
    };
    let composer_height = if panel.is_empty() { 1 } else { panel.len() as u16 };
    let working_height = app.working_since.is_some() as u16;
    let [live_area, working_area, top_rule, composer_area, bottom_rule, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(working_height),
        Constraint::Length(1),
        Constraint::Length(composer_height),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_live(frame, app, live_area);
    if let Some(started) = app.working_since {
        frame.render_widget(Paragraph::new(crate::working::line(started)), working_area);
    }
    // Shell mode: the rules turn red so `!` input is never mistaken for a
    // message.
    let rule_color = if app.shell_mode() { Color::Red } else { Color::DarkGray };
    render_rule(frame, top_rule, rule_color);
    if panel.is_empty() {
        render_input(frame, app, composer_area);
    } else {
        frame.render_widget(Paragraph::new(panel), composer_area);
    }
    render_rule(frame, bottom_rule, rule_color);
    crate::statusline::render(frame, app, status_area);
    // Drawn last so they sit over the live items, directly above the input.
    if app.picker.is_some() {
        render_picker(frame, app, live_area);
    } else {
        render_completion(frame, app, live_area);
    }
}

fn render_rule(frame: &mut Frame, area: Rect, color: Color) {
    let rule = "\u{2500}".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(rule, Style::default().fg(color))),
        area,
    );
}

/// Unfinished items, bottom-aligned. Only the newest rows fit; the rest
/// appear in scrollback once the item finishes.
fn render_live(frame: &mut Frame, app: &App, area: Rect) {
    let tools = LiveTool {
        pending: &app.pending_tools,
    };
    let width = area.width as usize;
    let lines: Vec<Line> = app
        .live_items()
        .iter()
        .flat_map(|item| crate::history::live_lines(item, width, &tools))
        .collect();
    let skip = lines.len().saturating_sub(area.height as usize);
    let shown: Vec<Line> = lines.into_iter().skip(skip).collect();
    let top = area.height.saturating_sub(shown.len() as u16);
    let area = Rect {
        y: area.y + top,
        height: area.height - top,
        ..area
    };
    frame.render_widget(Paragraph::new(shown), area);
}

/// Popup listing `items` as (name, description) rows above the input.
fn render_popup(frame: &mut Frame, area: Rect, title: &str, items: &[(String, String)], selected: usize) {
    if items.is_empty() || area.height < 3 {
        return;
    }
    let width = items
        .iter()
        .map(|(name, description)| name.width() + description.width() + 5)
        .chain([title.width() + 2])
        .max()
        .unwrap_or(20)
        .min(area.width as usize) as u16;
    let height = (items.len() as u16 + 2).min(area.height);
    let area = Rect {
        x: area.x,
        y: area.y + area.height - height,
        width,
        height,
    };
    let rows: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, (name, description))| {
            let mut name_style = Style::default().fg(Color::Cyan);
            if i == selected {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }
            let row = Line::from(vec![
                Span::styled(name.clone(), name_style),
                Span::raw("  "),
                Span::styled(description.clone(), Style::default().fg(Color::DarkGray)),
            ]);
            match i == selected {
                true => ListItem::new(row).style(Style::default().bg(Color::DarkGray)),
                false => ListItem::new(row),
            }
        })
        .collect();
    frame.render_widget(Clear, area);
    frame.render_widget(
        List::new(rows).block(Block::default().borders(Borders::ALL).title(title.to_string())),
        area,
    );
}

fn render_completion(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<(String, String)> = app
        .completion
        .matches(&app.input)
        .iter()
        .map(|m| (m.name.to_string(), m.description.to_string()))
        .collect();
    let selected = app.completion.selected(&app.input);
    render_popup(frame, area, "[Tab] complete  [Enter] run", &items, selected);
}

fn render_picker(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<(String, String)> = app
        .sessions
        .iter()
        .map(|s| (s.session_id.to_string()[..8].to_string(), s.model.clone()))
        .collect();
    let selected = app.picker.unwrap_or(0);
    render_popup(frame, area, "Resume  [Enter] open  [Esc] cancel", &items, selected);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let prompt = match app.shell_mode() {
        true => Span::styled("! ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        false => Span::styled("\u{276f} ", Style::default().add_modifier(Modifier::BOLD)),
    };
    let body = if app.input.is_empty() {
        let hint = match app.status {
            Status::Idle => "Message, /command, or !shell",
            _ => "working...",
        };
        Span::styled(hint, Style::default().fg(Color::DarkGray))
    } else {
        // In shell mode the `!` is the prompt itself.
        Span::raw(app.input.strip_prefix('!').unwrap_or(&app.input))
    };
    frame.render_widget(Paragraph::new(Line::from(vec![prompt, body])), area);

    // Place the cursor at the edit position, measured in display columns:
    // byte offsets and columns diverge for non-ASCII input.
    if app.status == Status::Idle {
        let offset = if app.shell_mode() { 1 } else { 2 };
        let cursor_x = area.x + offset + app.cursor_column() as u16;
        if cursor_x < area.x + area.width {
            frame.set_cursor_position((cursor_x, area.y));
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
