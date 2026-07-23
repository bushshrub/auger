use axum::Extension;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router, middleware};
use futures::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpListener;
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, info};
use uuid::Uuid;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::server_types::{
    ApproveRequest, CreateSessionRequest, SessionEntry, UserInputRequest,
};
use agent_core::{AutoApprovalPolicies, SessionBuilder, SessionCommand, SessionEvent, SessionHandle, SessionId, SystemPrompt, TraceReader, TraceWriter};
use provider::{LlmModel, LlmProvider};

mod server_types;
mod config;
mod provider_config;

fn trace_path(session_id: SessionId) -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME is not set");
    PathBuf::from(home)
        .join(".auger")
        .join("sessions")
        .join(session_id.to_string())
        .join("trace.jsonl")
}

const DEFAULT_CONTEXT_WINDOW: usize = 113072;
const SYSTEM_PROMPT: &str =
"You are a precise, capable software engineering agent. You have access to tools to read files, run commands, make changes, and search the web.

  Research first:
  - Before designing or implementing anything non-trivial, use web_search to look up relevant documentation, libraries, APIs, and prior art.
  - Use fetch_content to read the full text of any search result that looks relevant.
  - Only proceed to implementation after you understand the landscape.

  Guidelines:
  - Think before acting. For ambiguous tasks, clarify once then proceed.
  - Prefer small, targeted changes over large rewrites.
  - After making changes, verify they work (run tests, build, etc.).
  - If a tool call fails, diagnose before retrying.
  - When done, report what changed and why — not what you did step by step.
  - Never guess at file paths or API shapes. Read first.
 ";

#[derive(Clone)]
struct AppState {
    provider: Arc<dyn LlmProvider>,
    sessions: Arc<RwLock<HashMap<SessionId, SessionEntry>>>,
    disk_sessions: Arc<RwLock<HashMap<SessionId, DiskSession>>>,
    activation_lock: Arc<TokioMutex<()>>,
    default_model: String,
}

struct DiskSession {
    record: agent_core::SessionRecord,
    model: String,
    path: PathBuf,
    read_token: Uuid,
    write_token: Uuid,
}

fn load_disk_sessions() -> Vec<DiskSession> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let sessions_dir = PathBuf::from(home).join(".auger").join("sessions");
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("trace.jsonl");
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        match TraceReader::read(BufReader::new(file)) {
            Ok(record) => {
                let model = record.data().model_info().id().clone();
                sessions.push(DiskSession {
                    record,
                    model,
                    path,
                    read_token: Uuid::new_v4(),
                    write_token: Uuid::new_v4(),
                });
            }
            Err(error) => tracing::warn!(path = %path.display(), %error, "failed to restore session trace"),
        }
    }
    sessions
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::Config::load().unwrap_or_else(|error| panic!("{error}"));
    let addr = config.listen_addr();
    let provider = provider_config::from_config(&config);

    let disk_sessions = load_disk_sessions()
        .into_iter()
        .map(|session| (session.record.data().session_id(), session))
        .collect();
    let state = AppState {
        provider: Arc::clone(&provider),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        disk_sessions: Arc::new(RwLock::new(disk_sessions)),
        activation_lock: Arc::new(TokioMutex::new(())),
        default_model: config.model(),
    };

    let listener = TcpListener::bind(&addr).await.expect("bind failed");
    info!("agent-server listening on {addr}");
    axum::serve(listener, router(state))
        .await
        .expect("server error");
}

fn router(state: AppState) -> Router {
    let write_routes = Router::new()
        .route("/sessions/{id}", delete(stop_session))
        .route("/sessions/{id}/input", post(send_input))
        .route("/sessions/{id}/tool", post(respond_to_tool_call))
        .route("/sessions/{id}/interrupt", post(interrupt_session))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            resolve_session_extensions,
        ));

    let read_routes = Router::new()
        .route("/sessions/{id}/events", get(event_stream))
        .route("/sessions/{id}/snapshot", get(snapshot))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            resolve_session_extensions,
        ));


    Router::new()
        .route("/openapi.yaml", get(openapi))
        .route("/sessions", get(list_sessions).post(create_session))
        .merge(write_routes)
        .merge(read_routes)
        .with_state(state)
}

async fn resolve_session_extensions(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    mut request: Request,
    next: Next,
) -> Response {
    let entry = if let Some(entry) = state.sessions.read().await.get(&id).cloned() {
        Some(entry)
    } else {
        let _activation = state.activation_lock.lock().await;
        if let Some(entry) = state.sessions.read().await.get(&id).cloned() {
            Some(entry)
        } else if let Some(disk) = state.disk_sessions.write().await.remove(&id) {
            start_session(
                &state,
                SessionBuilder::restore(disk.record),
                disk.model,
                disk.path,
                Some((disk.read_token, disk.write_token)),
            ).await;
            state.sessions.read().await.get(&id).cloned()
        } else {
            None
        }
    };
    let Some(entry) = entry else {
        return StatusCode::NOT_FOUND.into_response();
    };

    request.extensions_mut().insert(entry.handle);
    request.extensions_mut().insert(entry.events);
    next.run(request).await
}

async fn openapi() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/yaml")],
        include_str!("../openapi.yaml"),
    )
}

/// List all sessions, including archived sessions.
async fn list_sessions(State(state): State<AppState>) -> impl IntoResponse {
    let sessions = state.sessions.read().await;
    let mut list: Vec<_> = sessions
        .values()
        .map(|e| {
            json!({
                "session_id": e.handle.id().as_uuid(),
                "model": e.model,
                "created_at": e.handle.created_at(),
                "archived": e.archived.load(Ordering::Relaxed),
                "context_window": DEFAULT_CONTEXT_WINDOW,
                "tokens": {
                    "read": e.read_token.to_string(),
                    "write": e.write_token.to_string(),
                }
            })
        })
        .collect();
    let disk_sessions = state.disk_sessions.read().await;
    list.extend(disk_sessions.values().map(|e| {
        json!({
            "session_id": e.record.data().session_id().as_uuid(),
            "model": e.model,
            "created_at": e.record.data().created_at(),
            "archived": false,
            "context_window": DEFAULT_CONTEXT_WINDOW,
            "tokens": {
                "read": e.read_token.to_string(),
                "write": e.write_token.to_string(),
            }
        })
    }));
    Json(json!({ "sessions": list }))
}

/// Request to create a session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let builder = SessionBuilder::new(model.clone());
    let session_id = builder.id();
    let (read_token, write_token) = start_session(&state, builder, model, trace_path(session_id), None).await;
    Json(
        json!({ "session_id": session_id, "context_window": DEFAULT_CONTEXT_WINDOW, "tokens": {
        "read": read_token,
        "write": write_token
    } }),
    )
}

async fn start_session(
    state: &AppState,
    builder: SessionBuilder,
    model: String,
    trace_path: PathBuf,
    tokens: Option<(Uuid, Uuid)>,
) -> (Uuid, Uuid) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let sys_prompt = SystemPrompt::new(SYSTEM_PROMPT.to_string()).inject_cwd();
    let tools: Vec<Box<dyn agent_tools::Tool>> = vec![
        Box::new(builtin_tools::ReadFile),
        Box::new(builtin_tools::WriteFile),
        Box::new(builtin_tools::EditFile),
        Box::new(builtin_tools::Glob),
        Box::new(builtin_tools::Grep),
        Box::new(builtin_tools::ListFiles),
        Box::new(builtin_tools::Shell),
        Box::new(builtin_tools::TodoList::new()),
        Box::new(builtin_tools::WebSearch::new()),
        Box::new(builtin_tools::FetchContent::new()),
    ];
    // Read-only tools and the conservative shell policy run without consent.
    let auto_approved: Vec<String> = [
        "read_file",
        "grep",
        "glob",
        "list_files",
        "todo_list",
        "web_search",
        "fetch_content",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut auto_approval = AutoApprovalPolicies::from(auto_approved);
    auto_approval.add("shell", builtin_tools::BashAutoApprovalPolicy::new(cwd.clone()));
    let session_id = builder.id();
    create_dir_all(trace_path.parent().expect("trace path has a parent"))
        .expect("failed to create session trace directory");
    let is_new_trace = !trace_path.exists()
        || std::fs::metadata(&trace_path).expect("failed to inspect session trace file").len() == 0;
    let trace_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&trace_path)
        .expect("failed to create session trace file");
    let writer = BufWriter::new(trace_file);
    let trace_writer = if is_new_trace {
        TraceWriter::new(writer, builder.session_data())
            .expect("failed to write session trace header")
    } else {
        TraceWriter::resume(writer)
    };
    let trace_writer = Arc::new(Mutex::new(trace_writer));
    let turn_writer = Arc::clone(&trace_writer);
    let event_writer = Arc::clone(&trace_writer);
    let builder = builder
        .on_turn(move |_, record| {
            match turn_writer.lock() {
                Ok(mut writer) => {
                    if let Err(error) = writer.write_turn(record) {
                        tracing::error!(%error, "failed to write session trace turn");
                    }
                }
                Err(_) => tracing::error!("trace writer lock poisoned"),
            }
        })
        .on_event(move |turn_id, record| {
            match event_writer.lock() {
                Ok(mut writer) => {
                    if let Err(error) = writer.write_event(turn_id, record) {
                        tracing::error!(%error, "failed to write session trace event");
                    }
                }
                Err(_) => tracing::error!("trace writer lock poisoned"),
            }
        });
    let (handle, event_rx) = builder.start(
        LlmModel::new(state.provider.clone(), &model),
        sys_prompt,
        tokio::runtime::Handle::current(),
        tools,
        auto_approval,
    );

    let (event_tx, _) = tokio::sync::broadcast::channel(256);
    let forward_tx = event_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = event_rx.recv() {
            let closed = matches!(event, SessionEvent::Closed);
            let _ = forward_tx.send(event);
            if closed {
                break;
            }
        }
    });
    let (read_token, write_token) = tokens.unwrap_or_else(|| (Uuid::new_v4(), Uuid::new_v4()));
    state.sessions.write().await.insert(
        session_id,
        SessionEntry {
            handle,
            events: event_tx,
            model,
            read_token,
            write_token,
            archived: Arc::new(AtomicBool::new(false)),
        },
    );
    (read_token, write_token)
}

async fn stop_session(State(state): State<AppState>, Path(id): Path<SessionId>) -> impl IntoResponse {
    let Some(entry) = state.sessions.read().await.get(&id).cloned() else {
        return StatusCode::NOT_FOUND;
    };
    let (reply_tx, reply_rx) = mpsc::channel::<>();
    tokio::task::spawn_blocking(move || entry.handle.send_command(SessionCommand::Stop { reply_tx })).await.ok();
    // do I wait for reply_rx? idk.
    entry.archived.store(true, Ordering::Release);
    StatusCode::NO_CONTENT
}

/// Submit input to the clanker
#[tracing::instrument(skip(session_handle, req))]
async fn send_input(
    Extension(session_handle): Extension<SessionHandle>,
    Json(req): Json<UserInputRequest>,
) -> impl IntoResponse {
    debug!("Enqueued message: '{}'", req.input);
    session_handle.send_command(SessionCommand::SendMessage(req.into())).expect("closed");

    // TODO: bad return type
    Json(json!({ "status": "ok" }))
}

/// Respond to a tool call
#[tracing::instrument(skip(session_handle, req), fields(session_id, call_id))]
async fn respond_to_tool_call(
    Extension(session_handle): Extension<SessionHandle>,
    Json(req): Json<ApproveRequest>,
) -> impl IntoResponse {
    let span = tracing::Span::current();
    span.record("session_id", tracing::field::display(&session_handle.id()));
    span.record("call_id", tracing::field::display(&req.tool_call_id));
    debug!("Tool call approved: {}", req.approved);
    let decision = SessionCommand::ToolDecision {
        id: req.tool_call_id,
        approved: req.approved,
        message: req.message,
    };
    session_handle
        .send_command(decision)
        .expect("closed");
    // TODO: garbage return type
    Json(json!({ "status": "ok" }))
}

/// Interrupt what the session is doing right now
#[tracing::instrument(skip(session_handle))]
async fn interrupt_session(
    Extension(session_handle): Extension<SessionHandle>,
) -> impl IntoResponse {
    debug!(session_id = %session_handle.id(), "Interrupt requested");
    session_handle.send_command(SessionCommand::Interrupt).expect("closed");
    Json(json!({ "status": "ok" }))
}

/// Get a trace from its running session or, after archival, its JSONL file.
async fn snapshot(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Extension(session_handle): Extension<SessionHandle>,
) -> impl IntoResponse {
    let archived = state
        .sessions
        .read()
        .await
        .get(&id)
        .is_some_and(|entry| entry.archived.load(Ordering::Relaxed));

    if archived {
        return (StatusCode::INTERNAL_SERVER_ERROR, "can't read archived yet").into_response()
    }

    let (snapshot_tx, snapshot_rx) = mpsc::channel();
    // what do I do with snapshot_rx?
    match tokio::task::spawn_blocking(move || session_handle.send_command(SessionCommand::Snapshot { reply_tx: snapshot_tx })).await {
        Ok(Ok(())) => match snapshot_rx.recv() {
            Ok(record) => {
                let mut trace = Vec::new();
                let result = (|| {
                    let mut writer = TraceWriter::new(&mut trace, record.data())?;
                    for turn in record.turns() {
                        writer.write_turn(turn)?;
                        for event in turn.events() {
                            writer.write_event(turn.data().turn_id(), event)?;
                        }
                    }
                    Ok::<_, agent_core::TraceWriteError>(())
                })();
                match result {
                    Ok(()) => (
                        [(CONTENT_TYPE, "application/x-ndjson")],
                        String::from_utf8(trace).expect("serialized JSON is UTF-8"),
                    ).into_response(),
                    Err(error) => {
                        tracing::error!(%error, "failed to serialize session snapshot");
                        (StatusCode::INTERNAL_SERVER_ERROR, "snapshot serialization failed").into_response()
                    }
                }
            }
            Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "snapshot channel dropped").into_response()
        },
        _ => (StatusCode::INTERNAL_SERVER_ERROR, "snapshot failed").into_response(),
    }
}

/// Get a stream of events for a session, including LLM responses and user events.
#[tracing::instrument(skip(event_tx))]
async fn event_stream(
    Extension(event_tx): Extension<tokio::sync::broadcast::Sender<SessionEvent>>,
) -> impl IntoResponse {
    let stream = BroadcastStream::new(event_tx.subscribe());
    Sse::new(stream.filter_map(|result| async move {
        let event = result.ok()?;
        let json = serde_json::to_string(&session_event_json(event)).ok()?;
        Some(Ok::<_, std::convert::Infallible>(
            axum::response::sse::Event::default().data(json),
        ))
    }))
}

fn session_event_json(event: SessionEvent) -> serde_json::Value {
    match event {
        SessionEvent::StreamEvent(event) => match event {
            provider::StreamEvent::TextDelta(text) => json!({ "type": "text_delta", "text": text }),
            provider::StreamEvent::ReasoningDelta(text) => json!({ "type": "reasoning_delta", "text": text }),
            provider::StreamEvent::ToolCall { id, name, arguments } => json!({
                "type": "tool_call",
                "id": id,
                "name": name,
                "arguments": arguments,
            }),
            provider::StreamEvent::ToolCallComplete { id, name, arguments } => json!({
                "type": "tool_call_complete",
                "id": id,
                "name": name,
                "arguments": arguments,
            }),
            provider::StreamEvent::Done { usage, stop_reason } => json!({
                "type": "done",
                "usage": usage,
                "stop_reason": stop_reason,
            }),
        },
        SessionEvent::ToolConsentRequired { tool_calls } => json!({
            "type": "tool_consent_required",
            "tool_calls": tool_calls,
        }),
        SessionEvent::ToolCallResult(result) => json!({
            "type": "tool_call_result",
            "id": result.tool_call_id(),
            "result": result,
        }),
        SessionEvent::Interrupted => json!({ "type": "interrupted" }),
        SessionEvent::StreamError { error } => json!({ "type": "stream_error", "error": error }),
        SessionEvent::Closed => json!({ "type": "closed" }),
    }
}
