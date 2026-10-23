//! Transcript pager (ctrl+t): the whole conversation on the alternate screen
//! with nothing folded, scrolled by the app. The inline view and the native
//! scrollback stay as they are underneath.

use crate::app::App;
use crossterm::event::DisableMouseCapture;
use crossterm::event::EnableMouseCapture;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use ratatui::DefaultTerminal;
use ratatui::Terminal;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::prelude::CrosstermBackend;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use std::io::Stdout;

pub struct Pager {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Pager {
    pub fn open() -> std::io::Result<Self> {
        crossterm::execute!(std::io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        let terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        Ok(Self { terminal })
    }

    /// Leave the alternate screen and repaint the inline view.
    pub fn close(self, inline: &mut DefaultTerminal) -> std::io::Result<()> {
        crossterm::execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
        inline.clear()
    }

    pub fn draw(&mut self, app: &App) -> std::io::Result<()> {
        self.terminal.draw(|frame| {
            let [body, footer] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
            let width = body.width as usize;
            let lines: Vec<Line> = app
                .items
                .iter()
                .flat_map(|item| crate::history::transcript_lines(item, width))
                .collect();
            let height = body.height as usize;
            let max = lines.len().saturating_sub(height);
            app.pager_max.set(max);
            let top = max - app.pager_from_bottom.min(max);
            let shown: Vec<Line> = lines.into_iter().skip(top).take(height).collect();
            frame.render_widget(Paragraph::new(shown), body);

            let dim = Style::default().fg(Color::DarkGray);
            let position = match max {
                0 => "all".to_string(),
                _ => format!("{}%", (top * 100) / max),
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" transcript ", Style::default().fg(Color::Black).bg(Color::Gray)),
                    Span::styled(
                        format!("  {position}  \u{2191}\u{2193}/wheel scroll \u{b7} g/G top/bottom \u{b7} q close"),
                        dim,
                    ),
                ])),
                footer,
            );
        })?;
        Ok(())
    }
}
