use axum::Extension;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
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
use tokio::sync::RwLock;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, info};
use uuid::Uuid;
use std::fs::{create_dir_all, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::server_types::{
    ApproveRequest, CreateSessionRequest, SessionEntry, UserInputRequest,
};
use agent_core::{AutoApprovalPolicies, SessionBuilder, SessionCommand, SessionEvent, SessionHandle, SessionId, SystemPrompt, TurnEvent};
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

fn write_trace_record(writer: &Arc<Mutex<BufWriter<std::fs::File>>>, record: &auger_traces::schema::TraceRecord) {
    let Ok(mut writer) = writer.lock() else {
        tracing::error!("trace writer lock poisoned");
        return;
    };
    let line = match serde_json::to_string(record) {
        Ok(line) => line,
        Err(error) => {
            tracing::error!(%error, "failed to serialize session trace");
            return;
        }
    };
    if let Err(error) = writer.write_all(line.as_bytes())
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
    {
        tracing::error!(%error, "failed to write session trace");
    }
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
    default_model: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = config::Config::load().unwrap_or_else(|error| panic!("{error}"));
    let addr = config.listen_addr();
    let provider = provider_config::from_config(&config);

    let state = AppState {
        provider: Arc::clone(&provider),
        sessions: Arc::new(RwLock::new(HashMap::new())),
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
        .route("/sessions/{id}/interrupt", post(interrupt_session));

    let read_routes = Router::new()
        .route("/sessions/{id}/events", get(event_stream))
        .route("/sessions/{id}/snapshot", get(snapshot));


    Router::new()
        .route("/openapi.yaml", get(openapi))
        .route("/sessions", get(list_sessions).post(create_session))
        .merge(write_routes)
        .merge(read_routes)
        .with_state(state)
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
    let list: Vec<_> = sessions
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
    Json(json!({ "sessions": list }))
}

/// Request to create a session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
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
    let builder = SessionBuilder::new(model.clone());
    let session_id = builder.id();
    let trace_path = trace_path(session_id);
    create_dir_all(trace_path.parent().expect("trace path has a parent"))
        .expect("failed to create session trace directory");
    let trace_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&trace_path)
        .expect("failed to create session trace file");
    let trace_writer = Arc::new(Mutex::new(BufWriter::new(trace_file)));
    if std::fs::metadata(&trace_path).expect("failed to inspect session trace file").len() == 0 {
        let root_turn_id: Uuid = builder.root_turn_id().into();
        let header = auger_traces::schema::SessionHeader::new(
            1,
            session_id.as_uuid(),
            auger_traces::schema::TurnId::from(root_turn_id),
            builder.created_at(),
            cwd.clone(),
            auger_traces::schema::ModelInfo::new("to-be-added".to_string(), model.clone()),
        );
        write_trace_record(&trace_writer, &auger_traces::schema::TraceRecord::Session(header));
    }
    let turn_writer = Arc::clone(&trace_writer);
    let event_writer = Arc::clone(&trace_writer);
    let builder = builder
        .on_turn(move |_, record| {
            write_trace_record(&turn_writer, &auger_traces::schema::TraceRecord::Turn(record.clone().into()));
        })
        .on_event(move |turn_id, record| {
            let event: auger_traces::schema::EventRecord =
                TurnEvent::new(turn_id, record.clone()).into();
            write_trace_record(&event_writer, &auger_traces::schema::TraceRecord::Event(event));
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
    let read_token = Uuid::new_v4();
    let write_token = Uuid::new_v4();
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
    Json(
        json!({ "session_id": session_id, "context_window": DEFAULT_CONTEXT_WINDOW, "tokens": {
        "read": read_token,
        "write": write_token
    } }),
    )
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
            // TODO: we should be sending back the schema, not the actual record.
            Ok(record) => Json(record).into_response(),
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
        SessionEvent::ToolCallResult { id, result } => json!({
            "type": "tool_call_result",
            "id": id,
            "result": result,
        }),
        SessionEvent::ToolCallError { id, error } => json!({
            "type": "tool_call_error",
            "id": id,
            "error": error,
        }),
        SessionEvent::Interrupted => json!({ "type": "interrupted" }),
        SessionEvent::StreamError { error } => json!({ "type": "stream_error", "error": error }),
        SessionEvent::Closed => json!({ "type": "closed" }),
    }
}
