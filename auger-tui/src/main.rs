use std::time::{Duration, Instant};

use crossterm::{
    event::{Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use tokio::sync::mpsc;

mod api;
mod app;
mod types;
mod ui;

use app::{App, View};
use types::{AppEvent, TuiEvent};

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

    let mut app = App::new(server.clone());

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
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = ratatui::init();

    let result = run(&mut terminal, &mut app, &mut rx, &tx, &server).await;

    // Restore terminal
    ratatui::restore();
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
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
    let Event::Key(key) = ev else { return };
    if key.kind != KeyEventKind::Press {
        return;
    }

    match app.view {
        View::SessionList => handle_session_list_key(key, app, tx, server).await,
        View::Chat => handle_chat_key(key, app, tx, server).await,
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
        KeyCode::End => app.scroll_to_bottom(),

        // Tool approval
        KeyCode::Char('y') if app.pending_tool_id.is_some() => {
            if let Some((sid, write_token, tool_id)) = app.approve_tool(true) {
                let s = server.to_string();
                let app_tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        api::respond_to_tool(&s, sid, &write_token, &tool_id, true, None).await
                    {
                        let _ = app_tx
                            .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                            .await;
                    }
                });
            }
        }
        KeyCode::Char('n') if app.pending_tool_id.is_some() => {
            if let Some((sid, write_token, tool_id)) = app.approve_tool(false) {
                let s = server.to_string();
                let app_tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        api::respond_to_tool(&s, sid, &write_token, &tool_id, false, None).await
                    {
                        let _ = app_tx
                            .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                            .await;
                    }
                });
            }
        }

        // Text input
        KeyCode::Char(c) if app.pending_tool_id.is_none() => {
            app.input.push(c);
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Enter => {
            if app.pending_tool_id.is_none() {
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
