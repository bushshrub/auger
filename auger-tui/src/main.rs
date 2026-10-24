use crossterm::event::DisableMouseCapture;
use crossterm::event::EnableMouseCapture;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use crossterm::execute;
use crossterm::terminal::EnterAlternateScreen;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::terminal::disable_raw_mode;
use crossterm::terminal::enable_raw_mode;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;

mod api;
mod app;
mod command;
mod completion;
mod diff;
mod markdown;
mod toolargs;
mod types;
mod ui;

use app::App;
use app::View;
use command::Command;
use types::AppEvent;
use types::TuiEvent;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = std::env::args()
        .find(|a| a.starts_with("--server="))
        .and_then(|a| a.strip_prefix("--server=").map(|s| s.to_string()))
        .or_else(|| {
            let mut args = std::env::args().skip(1).peekable();
            while let Some(a) = args.next() {
                if a == "--server" {
                    return args.next();
                }
            }
            None
        })
        .unwrap_or_else(|| "http://127.0.0.1:3000".to_string());

    let mut app = App::new();

    // Unified event channel: terminal input + async app events.
    // Large buffer so fast SSE streams don't block the producer.
    let (tx, mut rx) = mpsc::channel::<TuiEvent>(256);

    // Spawn terminal event reader
    let term_tx = tx.clone();
    tokio::task::spawn_blocking(move || {
        loop {
            if crossterm::event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(ev) = crossterm::event::read() {
                    if term_tx.blocking_send(TuiEvent::Terminal(ev)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Load sessions on startup
    let startup_tx = tx.clone();
    let startup_server = server.clone();
    tokio::spawn(async move {
        match api::list_sessions(&startup_server).await {
            Ok(sessions) => {
                let _ = startup_tx
                    .send(TuiEvent::App(AppEvent::SessionsLoaded(sessions)))
                    .await;
            }
            Err(e) => {
                let _ = startup_tx
                    .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                    .await;
            }
        }
    });

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    // Mouse capture drives click-to-expand and wheel scrolling. It also takes
    // over the terminal's own selection; most terminals still select on
    // shift-drag.
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = ratatui::init();

    let result = run(&mut terminal, &mut app, &mut rx, &tx, &server).await;

    // Restore terminal
    ratatui::restore();
    execute!(std::io::stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;

    result
}

const FRAME_BUDGET: Duration = Duration::from_millis(16); // ~60 fps cap

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    rx: &mut mpsc::Receiver<TuiEvent>,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        // Wait for the first event, then drain everything that arrived in the
        // same frame window before drawing again. This batches bursts of fast
        // SSE content-delta events so we only redraw once per batch.
        let deadline = Instant::now() + FRAME_BUDGET;
        tokio::select! {
            maybe = rx.recv() => {
                let Some(ev) = maybe else { break };
                process_event(ev, app, tx, server).await;
                // Drain any additional events that are already in the buffer.
                while let Ok(ev) = rx.try_recv() {
                    process_event(ev, app, tx, server).await;
                    if Instant::now() >= deadline {
                        break; // don't starve the renderer
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline.into()) => {}
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn process_event(ev: TuiEvent, app: &mut App, tx: &mpsc::Sender<TuiEvent>, server: &str) {
    match ev {
        TuiEvent::App(app_ev) => {
            let start_stream = matches!(&app_ev, AppEvent::SessionCreated { .. });
            app.handle_app_event(app_ev);
            if start_stream {
                if let (Some(sid), Some(token)) = (app.session_id, app.read_token.clone()) {
                    api::spawn_event_stream(server.to_string(), sid, token, tx.clone());
                }
            }
        }
        TuiEvent::Terminal(ev) => {
            handle_terminal_event(ev, app, tx, server).await;
        }
    }
}

async fn handle_terminal_event(
    ev: Event,
    app: &mut App,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match app.view {
            View::SessionList => handle_session_list_key(key, app, tx, server).await,
            View::Chat => handle_chat_key(key, app, tx, server).await,
        },
        Event::Mouse(mouse) if app.view == View::Chat => handle_chat_mouse(mouse, app),
        _ => {}
    }
}

/// Execute a slash command. These act locally or against the API, and are
/// never forwarded to the model as chat input.
async fn run_command(command: Command, app: &mut App, tx: &mpsc::Sender<TuiEvent>, server: &str) {
    match command {
        Command::New { model } => {
            app.push_notice(match &model {
                Some(m) => format!("Starting a new session on {m}…"),
                None => "Starting a new session…".to_string(),
            });
            spawn_create_session(model, tx, server);
        }

        Command::Model { name } => match name {
            // The API has no way to change a running session's model, so the
            // only honest action is to start a fresh one.
            Some(model) => {
                app.push_notice(format!("Starting a new session on {model}…"));
                spawn_create_session(Some(model), tx, server);
            }
            None => {
                let current = app
                    .session_id
                    .and_then(|id| app.sessions.iter().find(|s| s.session_id == id))
                    .map(|s| s.model.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                app.push_notice(format!(
                    "Model: {current}\n\n\
                     `/model <name>` starts a *new* session — a running session's \
                     model can't be changed."
                ));
            }
        },

        Command::Sessions => {
            app.view = View::SessionList;
            refresh_sessions(tx, server);
        }

        Command::Help => app.push_notice(command::help_text()),

        Command::Quit => app.should_quit = true,

        Command::Unknown { name } => {
            app.push_error(format!("Unknown command: /{name} — try /help"));
        }
    }
}

fn spawn_create_session(model: Option<String>, tx: &mpsc::Sender<TuiEvent>, server: &str) {
    let app_tx = tx.clone();
    let server = server.to_string();
    tokio::spawn(async move {
        let event = match api::create_session(&server, model.as_deref()).await {
            Ok(ev) => ev,
            Err(e) => AppEvent::NetworkError(e.to_string()),
        };
        let _ = app_tx.send(TuiEvent::App(event)).await;
    });
}

fn refresh_sessions(tx: &mpsc::Sender<TuiEvent>, server: &str) {
    let app_tx = tx.clone();
    let server = server.to_string();
    tokio::spawn(async move {
        let event = match api::list_sessions(&server).await {
            Ok(sessions) => AppEvent::SessionsLoaded(sessions),
            Err(e) => AppEvent::NetworkError(e.to_string()),
        };
        let _ = app_tx.send(TuiEvent::App(event)).await;
    });
}

/// Send a decision for each of `tool_ids`. Every queued call needs its own
/// response or the session sits waiting forever.
fn respond_to_tools(
    session_id: uuid::Uuid,
    write_token: String,
    tool_ids: Vec<String>,
    approved: bool,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) {
    let server = server.to_string();
    let app_tx = tx.clone();
    tokio::spawn(async move {
        for tool_id in tool_ids {
            if let Err(e) = api::respond_to_tool(
                &server,
                session_id,
                &write_token,
                &tool_id,
                approved,
                None,
            )
            .await
            {
                let _ = app_tx
                    .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                    .await;
            }
        }
    });
}

fn handle_chat_mouse(mouse: MouseEvent, app: &mut App) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            app.click(mouse.column, mouse.row);
        }
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        _ => {}
    }
}

async fn handle_session_list_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) {
    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Down | KeyCode::Char('j') => app.session_list_next(),
        KeyCode::Up | KeyCode::Char('k') => app.session_list_prev(),
        KeyCode::Enter => {
            if let Some(info) = app.selected_session().cloned() {
                app.open_session(&info);
                // spawn_event_stream loads snapshot first, then subscribes to live events
                api::spawn_event_stream(
                    server.to_string(),
                    info.session_id,
                    info.read_token.clone(),
                    tx.clone(),
                );
            }
        }
        KeyCode::Char('n') => {
            let app_tx = tx.clone();
            let s = server.to_string();
            tokio::spawn(async move {
                match api::create_session(&s, None).await {
                    Ok(ev) => {
                        let _ = app_tx.send(TuiEvent::App(ev)).await;
                    }
                    Err(e) => {
                        let _ = app_tx
                            .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                            .await;
                    }
                }
            });
        }
        _ => {}
    }
}

async fn handle_chat_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) {
    match key.code {
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        // The slash-command popup owns Up/Down/Tab/Esc while it is open, so
        // the same keys don't also scroll or leave the chat.
        KeyCode::Up if app.completion_open() => app.completion.prev(&app.input),
        KeyCode::Down if app.completion_open() => app.completion.next(&app.input),
        KeyCode::Tab if app.completion_open() => {
            app.accept_completion();
        }
        KeyCode::Esc if app.completion_open() => app.completion.dismiss(),

        KeyCode::Esc => {
            app.view = View::SessionList;
            // Refresh session list
            let app_tx = tx.clone();
            let s = server.to_string();
            tokio::spawn(async move {
                match api::list_sessions(&s).await {
                    Ok(sessions) => {
                        let _ = app_tx
                            .send(TuiEvent::App(AppEvent::SessionsLoaded(sessions)))
                            .await;
                    }
                    Err(e) => {
                        let _ = app_tx
                            .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                            .await;
                    }
                }
            });
        }
        KeyCode::Up => app.scroll_up(3),
        KeyCode::Down => app.scroll_down(3),
        KeyCode::PageUp => app.scroll_up(20),
        KeyCode::PageDown => app.scroll_down(20),
        // End edits the input line; jumping the transcript to the bottom moves
        // to Ctrl+End so both are reachable.
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_to_bottom(),
        KeyCode::End => app.cursor_end(),

        // Tool approval. A turn can ask for several calls at once and the
        // server needs a response for every one of them, so y/n answer the
        // head of the queue and a/d answer the rest in one go.
        KeyCode::Char('y') if app.has_pending_tool() => {
            if let Some((sid, token, id)) = app.decide_tool(true) {
                respond_to_tools(sid, token, vec![id], true, tx, server);
            }
        }
        KeyCode::Char('n') if app.has_pending_tool() => {
            if let Some((sid, token, id)) = app.decide_tool(false) {
                respond_to_tools(sid, token, vec![id], false, tx, server);
            }
        }
        KeyCode::Char('a') if app.has_pending_tool() => {
            if let Some((sid, token, ids)) = app.decide_all_tools(true) {
                respond_to_tools(sid, token, ids, true, tx, server);
            }
        }
        KeyCode::Char('d') if app.has_pending_tool() => {
            if let Some((sid, token, ids)) = app.decide_all_tools(false) {
                respond_to_tools(sid, token, ids, false, tx, server);
            }
        }

        // Text input and cursor movement
        KeyCode::Char(c) if !app.has_pending_tool() => {
            app.insert_char(c);
        }
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete(),
        KeyCode::Left => app.cursor_left(),
        KeyCode::Right => app.cursor_right(),
        KeyCode::Home => app.cursor_home(),
        KeyCode::Enter => {
            // Enter on the popup takes the highlighted command and runs it.
            if app.completion_open() {
                app.accept_completion();
            }
            // Slash commands are handled locally and never sent to the model.
            if !app.has_pending_tool() {
                if let Some(command) = app.take_command() {
                    run_command(command, app, tx, server).await;
                    return;
                }
            }
            if !app.has_pending_tool() {
                if let Some((sid, write_token, text)) = app.send_message() {
                    let s = server.to_string();
                    let app_tx = tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = api::send_input(&s, sid, &write_token, &text).await {
                            let _ = app_tx
                                .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                                .await;
                        }
                    });
                }
            }
        }

        _ => {}
    }
}
