use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::TerminalOptions;
use ratatui::Viewport;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;

mod api;
mod app;
mod approval;
mod command;
mod completion;
mod config;
mod diff;
mod history;
mod markdown;
mod shell;
mod statusline;
mod toolargs;
mod types;
mod ui;
mod working;

use app::App;
use approval::Choice;
use types::Status;
use command::Command;
use types::AppEvent;
use types::TuiEvent;

/// Run the agent server in this process on a free local port and return its URL.
async fn start_embedded_server() -> anyhow::Result<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(error) = agent_server::serve(listener).await {
            eprintln!("embedded agent server stopped: {error}");
        }
    });
    Ok(format!("http://{addr}"))
}

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
        });
    let server = match server {
        Some(server) => server,
        None => start_embedded_server().await?,
    };

    let mut app = App::new();
    app.statusline = config::TuiConfig::load()?.statusline;

    // Unified event channel: terminal input + async app events.
    // Large buffer so fast SSE streams don't block the producer.
    let (tx, mut rx) = mpsc::channel::<TuiEvent>(256);

    // Spawn terminal event reader
    let term_tx = tx.clone();
    // Stops once the app drops the receiver; the runtime waits for this
    // thread on shutdown.
    tokio::task::spawn_blocking(move || {
        while !term_tx.is_closed() {
            if shell::INPUT_PAUSED.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if crossterm::event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(ev) = crossterm::event::read() {
                    if term_tx.blocking_send(TuiEvent::Terminal(ev)).is_err() {
                        break;
                    }
                }
            }
        }
    });

    spawn_create_session(None, &tx, &server);

    // In raw mode Ctrl-C is a key, but while a `!` command runs the terminal
    // is cooked and SIGINT reaches this process too. Swallow it so only the
    // command stops.
    tokio::spawn(async {
        while tokio::signal::ctrl_c().await.is_ok() {}
    });

    // Inline viewport: finished items are printed above it into the
    // terminal's own scrollback.
    let mut terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(ui::VIEWPORT_HEIGHT),
    });

    let result = run(&mut terminal, &mut app, &mut rx, &tx, &server).await;

    // Leave the transcript in scrollback and the shell prompt below it.
    let _ = terminal.clear();
    ratatui::restore();

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
        ui::flush_finished(terminal, app)?;
        if let Some(command) = app.shell_request.take() {
            shell::run(terminal, &command, ui::VIEWPORT_HEIGHT)?;
        }
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

        app.track_working();
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
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if app.picker.is_some() {
                handle_picker_key(key, app, tx, server);
            } else {
                handle_chat_key(key, app, tx, server).await;
            }
        }
        _ => {}
    }
}

/// Execute a slash command. These act locally or against the API, and are
/// never forwarded to the model as chat input.
async fn run_command(command: Command, app: &mut App, tx: &mpsc::Sender<TuiEvent>, server: &str) {
    match command {
        Command::New { model } => {
            app.push_notice(match &model {
                Some(m) => format!("Starting a new session on {m}..."),
                None => "Starting a new session...".to_string(),
            });
            spawn_create_session(model, tx, server);
        }

        Command::Model { name } => match name {
            // The API has no way to change a running session's model, so the
            // only honest action is to start a fresh one.
            Some(model) => {
                app.push_notice(format!("Starting a new session on {model}..."));
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
                     `/model <name>` starts a *new* session; a running session's \
                     model can't be changed."
                ));
            }
        },

        Command::Resume => refresh_sessions(tx, server),

        Command::Help => app.push_notice(command::help_text()),

        Command::Quit => app.should_quit = true,

        Command::Unknown { name } => {
            app.push_error(format!("Unknown command: /{name}, try /help"));
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

/// Keys while the approval panel is open: arrows and Enter pick a row,
/// y/a/n/d and Esc choose directly. Every queued call needs its own answer.
fn handle_approval_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) {
    let choice = match key.code {
        KeyCode::Up | KeyCode::Char('k') => return app.approval_prev(),
        KeyCode::Down | KeyCode::Char('j') => return app.approval_next(),
        KeyCode::Enter => app.approval_choice(),
        KeyCode::Char('y') => Some(Choice::Approve),
        KeyCode::Char('a') => Some(Choice::ApproveAll),
        KeyCode::Char('n') | KeyCode::Esc => Some(Choice::Deny),
        KeyCode::Char('d') => Some(Choice::DenyAll),
        _ => None,
    };
    if let Some((sid, token, ids)) = choice.and_then(|c| app.decide(c).map(|d| (d, c))).map(
        |((sid, token, ids), c)| (sid, token, (ids, c.approved())),
    ) {
        let (ids, approved) = ids;
        respond_to_tools(sid, token, ids, approved, tx, server);
    }
}

/// Keys while the `/resume` picker is open.
fn handle_picker_key(
    key: crossterm::event::KeyEvent,
    app: &mut App,
    tx: &mpsc::Sender<TuiEvent>,
    server: &str,
) {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Down | KeyCode::Char('j') => app.picker_next(),
        KeyCode::Up | KeyCode::Char('k') => app.picker_prev(),
        KeyCode::Esc => app.picker = None,
        KeyCode::Enter => {
            if let Some(info) = app.take_picked() {
                app.open_session(&info);
                // Loads the snapshot first, then subscribes to live events.
                api::spawn_event_stream(
                    server.to_string(),
                    info.session_id,
                    info.read_token.clone(),
                    tx.clone(),
                );
            }
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
        // The approval panel owns every other key while it is open.
        _ if app.has_pending_tool() => handle_approval_key(key, app, tx, server),
        KeyCode::Esc if app.status == Status::Running => {
            if let (Some(sid), Some(token)) = (app.session_id, app.write_token.clone()) {
                let server = server.to_string();
                tokio::spawn(async move {
                    let _ = api::interrupt(&server, sid, &token).await;
                });
            }
        }

        KeyCode::End => app.cursor_end(),

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
            // `!command` runs on the terminal; the main loop picks it up.
            if app.status == Status::Idle && app.take_shell() {
                return;
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
