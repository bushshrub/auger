//! `!command`: run a shell command on the real terminal.
//!
//! The TUI steps aside completely while the command runs: the viewport is
//! cleared, raw mode is off and the input reader is paused, so the command
//! owns the tty. Passwords, pagers and interactive programs work. Its output
//! lands in the terminal's scrollback like everything else, and a fresh
//! inline viewport is opened below it afterwards.

use crossterm::style::Stylize;
use ratatui::DefaultTerminal;
use ratatui::Terminal;
use ratatui::TerminalOptions;
use ratatui::Viewport;
use ratatui::prelude::CrosstermBackend;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Set while a command owns the terminal; the input reader stops reading.
pub static INPUT_PAUSED: AtomicBool = AtomicBool::new(false);

/// How long the input reader may still be inside one poll.
const READER_POLL: Duration = Duration::from_millis(60);

/// Run `command` with `$SHELL -c` on the terminal, then reopen the viewport.
pub fn run(terminal: &mut DefaultTerminal, command: &str, height: u16) -> std::io::Result<()> {
    INPUT_PAUSED.store(true, Ordering::SeqCst);
    std::thread::sleep(READER_POLL);

    let top = terminal.get_frame().area().y;
    terminal.clear()?;
    terminal.set_cursor_position((0, top))?;
    crossterm::terminal::disable_raw_mode()?;

    let mut stdout = std::io::stdout();
    writeln!(stdout, "{}", format!("! {command}").red().bold())?;
    stdout.flush()?;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let status = std::process::Command::new(shell).arg("-c").arg(command).status();
    match status {
        Ok(status) if !status.success() => {
            writeln!(stdout, "{}", format!("exited with {status}").dark_grey())?
        }
        Err(error) => writeln!(stdout, "{}", format!("failed to run: {error}").red())?,
        Ok(_) => {}
    }
    writeln!(stdout)?;

    crossterm::terminal::enable_raw_mode()?;
    *terminal = Terminal::with_options(
        CrosstermBackend::new(std::io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )?;
    INPUT_PAUSED.store(false, Ordering::SeqCst);
    Ok(())
}
