//! The bottom statusline. Which fields show, and in what order, comes from
//! `statusline` in the `[tui]` config table.

use crate::app::App;
use crate::config::StatusItem;
use crate::types::Status;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

const SEPARATOR: &str = " \u{b7} ";

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![];
    for item in &app.statusline {
        let Some(span) = field(*item, app) else { continue };
        if !spans.is_empty() {
            spans.push(Span::styled(SEPARATOR, Style::default().fg(Color::DarkGray)));
        }
        spans.push(span);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// One field, or `None` when there is nothing to show for it.
fn field(item: StatusItem, app: &App) -> Option<Span<'static>> {
    let span = match item {
        StatusItem::Model if app.model.is_empty() => return None,
        StatusItem::Model => Span::styled(app.model.clone(), Style::default().fg(Color::Magenta)),
        // Auger has no reasoning effort setting yet.
        StatusItem::Effort => return None,
        StatusItem::Input => Span::raw(format!(
            "\u{2191} {} ({} cached)",
            fmt_tokens(app.tokens_in),
            fmt_tokens(app.tokens_cached)
        )),
        StatusItem::Output => Span::raw(format!("\u{2193} {}", fmt_tokens(app.tokens_out_live()))),
        StatusItem::Status => match app.status {
            Status::Idle => Span::styled("idle", Style::default().fg(Color::Green)),
            Status::Running => Span::styled("working", Style::default().fg(Color::Yellow)),
            Status::Connecting => Span::styled("connecting", Style::default().fg(Color::Blue)),
        },
        StatusItem::Context if app.ctx_window == 0 => return None,
        StatusItem::Context => {
            let pct = (app.ctx_used as f64 / app.ctx_window as f64 * 100.0).min(100.0);
            let color = match pct {
                p if p >= 90.0 => Color::Red,
                p if p >= 75.0 => Color::Yellow,
                _ => Color::DarkGray,
            };
            Span::styled(
                format!("{}/{} ({:.0}%)", fmt_tokens(app.ctx_used), fmt_tokens(app.ctx_window), pct),
                Style::default().fg(color),
            )
        }
    };
    Some(span)
}

/// Compact token count: 812, 1.2k, 45.6k, 1.3M.
fn fmt_tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1_000.0),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}
