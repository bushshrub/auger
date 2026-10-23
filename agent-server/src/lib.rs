use crate::server_types::ApproveRequest;
use crate::stream_events::BlockTracker;
use crate::server_types::CreateSessionRequest;
use crate::server_types::SessionEntry;
use crate::server_types::UserInputRequest;
use auger_core::SessionBuilder;
use auger_core::SessionCommand;
use auger_core::SessionEvent;
use auger_core::SessionHandle;
use auger_core::SessionId;
use auger_core::Record;
use auger_core::RestoreError;
use auger_core::SystemPrompt;
use axum::Extension;
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::middleware;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::Sse;
use axum::routing::delete;
use axum::routing::get;
use axum::routing::post;
use futures::StreamExt;
use provider::LlmModel;
use provider::LlmProvider;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use tokio::net::TcpListener;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::RwLock;
use tokio_stream::wrappers::BroadcastStream;
use tracing::debug;
use tracing::info;
use tracing::warn;
use uuid::Uuid;

mod config;
mod provider_config;
mod server_types;
mod stream_events;
mod trace_store;

const DEFAULT_CONTEXT_WINDOW: usize = 113072;
const SYSTEM_PROMPT: &str = "You are a precise, capable software engineering agent. Use the \
                             tools you are given to inspect and change the project.

  Guidelines:
  - Think before acting. For ambiguous tasks, clarify once then proceed.
  - Prefer small, targeted changes over large rewrites.
  - After making changes, verify they work.
  - If a tool call fails, diagnose before retrying.
  - When done, report what changed and why - not what you did step by step.
  - Never guess at file paths or API shapes. Read first.
 ";

#[derive(Clone)]
struct AppState {
    provider: Arc<dyn LlmProvider>,
    sessions: Arc<RwLock<HashMap<SessionId, SessionEntry>>>,
    disk_sessions: Arc<RwLock<HashMap<SessionId, DiskSession>>>,
    activation_lock: Arc<TokioMutex<()>>,
    default_model: String,
    sessions_dir: Option<PathBuf>,
    mcp_servers: Arc<BTreeMap<String, auger_mcp::ServerConfig>>,
}

/// A session stored on disk, started again on its first input.
struct DiskSession {
    header: auger_core::SessionHeader,
    records: Vec<Record>,
    model: String,
    read_token: Uuid,
    write_token: Uuid,
    archived: bool,
    events: tokio::sync::broadcast::Sender<SessionEvent>,
}

/// The address from the config or `LISTEN_ADDR`.
pub fn listen_addr() -> Result<String, String> {
    Ok(config::Config::load()?.listen_addr())
}

/// What an embedder needs to run sessions the way the server does.
pub struct Defaults {
    pub provider: Arc<dyn LlmProvider>,
    pub model: String,
    pub sessions_dir: Option<PathBuf>,
    pub system_prompt: String,
    pub mcp_servers: BTreeMap<String, auger_mcp::ServerConfig>,
}

/// The configured provider and defaults, read from the same config as the server.
pub fn defaults() -> Result<Defaults, String> {
    let config = config::Config::load()?;
    Ok(Defaults {
        provider: provider_config::from_config(&config),
        model: config.model(),
        sessions_dir: trace_store::sessions_dir(),
        system_prompt: SYSTEM_PROMPT.to_string(),
        mcp_servers: config.mcp.servers.clone(),
    })
}

/// Serve the agent API on `listener` until the server fails.
pub async fn serve(listener: TcpListener) -> Result<(), String> {
    let router = api_router()?;
    if let Ok(addr) = listener.local_addr() {
        info!("agent-server listening on {addr}");
    }
    axum::serve(listener, router)
        .await
        .map_err(|error| format!("server error: {error}"))
}

/// The agent API routes with their state, configured the way `serve` configures them.
pub fn api_router() -> Result<Router, String> {
    let config = config::Config::load()?;
    let provider = provider_config::from_config(&config);

    let sessions_dir = trace_store::sessions_dir();
    let mut disk_sessions = HashMap::new();
    for (header, records) in sessions_dir.as_deref().map(trace_store::load_all).unwrap_or_default() {
        disk_sessions.insert(header.data.session_id(), DiskSession {
            model: header.data.model_info().id().clone(),
            header,
            records,
            read_token: Uuid::new_v4(),
            write_token: Uuid::new_v4(),
            archived: false,
            events: tokio::sync::broadcast::channel(256).0,
        });
    }
    info!(count = disk_sessions.len(), "loaded stored sessions");

    let state = AppState {
        provider: Arc::clone(&provider),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        disk_sessions: Arc::new(RwLock::new(disk_sessions)),
        activation_lock: Arc::new(TokioMutex::new(())),
        default_model: config.model(),
        sessions_dir,
        mcp_servers: Arc::new(config.mcp.servers.clone()),
    };

    Ok(router(state))
}

fn router(state: AppState) -> Router {
    let activation_routes = Router::new()
        .route("/sessions/{id}/input", post(send_input))
        .route("/sessions/{id}/tool", post(respond_to_tool_call))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            resolve_session_extensions,
        ));

    Router::new()
        .route("/openapi.yaml", get(openapi))
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", delete(stop_session))
        .route("/sessions/{id}/interrupt", post(interrupt_session))
        .route("/sessions/{id}/events", get(event_stream))
        .route("/sessions/{id}/snapshot", get(snapshot))
        .merge(activation_routes)
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
        } else {
            let disk = state.disk_sessions.write().await.remove(&id);
            if let Some(disk) = disk {
                let started = match SessionBuilder::restore(disk.records.clone()) {
                    Ok(builder) => start_session(
                        &state,
                        builder,
                        disk.model.clone(),
                        Some((disk.read_token, disk.write_token)),
                        Some(disk.events.clone()),
                        disk.records.clone(),
                    )
                    .await
                    .map(|_| ()),
                    Err(error) => Err(error),
                };
                if let Err(error) = started {
                    warn!(%error, session_id = %id, "cannot restore session");
                    state.disk_sessions.write().await.insert(id, disk);
                    return (StatusCode::CONFLICT, format!("cannot restore session: {error}"))
                        .into_response();
                }
                state.sessions.read().await.get(&id).cloned()
            } else {
                None
            }
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
#[tracing::instrument(skip(state))]
async fn list_sessions(State(state): State<AppState>) -> impl IntoResponse {
    info!("listing sessions");
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
            "session_id": e.header.data.session_id().as_uuid(),
            "model": e.model,
            "created_at": e.header.data.created_at(),
            "archived": e.archived,
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
    let sys_prompt = SystemPrompt::new(SYSTEM_PROMPT.to_string()).inject_cwd();
    let builder = SessionBuilder::new(model.clone(), sys_prompt);
    let session_id = builder.id();
    let (read_token, write_token) =
        start_session(&state, builder, model.clone(), None, None, Vec::new())
            .await
            .expect("a new session always starts");
    Json(
        json!({ "session_id": session_id, "model": model, "context_window": DEFAULT_CONTEXT_WINDOW, "tokens": {
        "read": read_token,
        "write": write_token
    } }),
    )
}

async fn start_session(
    state: &AppState,
    builder: SessionBuilder,
    model: String,
    tokens: Option<(Uuid, Uuid)>,
    events: Option<tokio::sync::broadcast::Sender<SessionEvent>>,
    records: Vec<Record>,
) -> Result<(Uuid, Uuid), RestoreError> {
    let cwd = builder.session_data().cwd().clone();
    let (mcp_tools, mcp_policy) = auger_mcp::McpServers::start(&state.mcp_servers, &cwd)
        .await
        .into_parts();
    let mut tools: Vec<Box<dyn auger_toolengine::Tool>> = vec![
        Box::new(builtin_tools::ReadFile),
        Box::new(builtin_tools::WriteFile),
        Box::new(builtin_tools::EditFile),
        Box::new(builtin_tools::Shell),
    ];
    tools.extend(mcp_tools);
    let policy = auger_toolengine::Chain(mcp_policy, auger_policy::default_policy());
    let session_id = builder.id();
    let log = Arc::new(std::sync::Mutex::new(records));
    let sink = trace_store::ServerSink::open(state.sessions_dir.as_deref(), session_id, log.clone());
    let (handle, event_rx) = builder.sink(sink).start(
        LlmModel::new(state.provider.clone(), &model),
        tokio::runtime::Handle::current(),
        tools,
        std::sync::Arc::new(policy),
    )?;

    let event_tx = events.unwrap_or_else(|| tokio::sync::broadcast::channel(256).0);
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
            records: log,
            model,
            read_token,
            write_token,
            archived: Arc::new(AtomicBool::new(false)),
        },
    );
    Ok((read_token, write_token))
}

async fn stop_session(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
) -> impl IntoResponse {
    if let Some(entry) = state.sessions.read().await.get(&id).cloned() {
        let (reply_tx, _reply_rx) = mpsc::channel();
        tokio::task::spawn_blocking(move || {
            entry.handle.send_command(SessionCommand::Stop { reply_tx })
        })
        .await
        .ok();
        // do I wait for reply_rx? idk.
        entry.archived.store(true, Ordering::Release);
        return StatusCode::NO_CONTENT;
    }
    if let Some(entry) = state.disk_sessions.write().await.get_mut(&id) {
        entry.archived = true;
        return StatusCode::NO_CONTENT;
    }
    StatusCode::NOT_FOUND
}

/// Submit input to the clanker
#[tracing::instrument(skip(session_handle, req))]
async fn send_input(
    Extension(session_handle): Extension<SessionHandle>,
    Json(req): Json<UserInputRequest>,
) -> impl IntoResponse {
    debug!("Enqueued message: '{}'", req.input);
    session_handle
        .send_command(SessionCommand::SendMessage(req.into()))
        .expect("closed");

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
    session_handle.send_command(decision).expect("closed");
    // TODO: garbage return type
    Json(json!({ "status": "ok" }))
}

/// Interrupt what the session is doing right now
#[tracing::instrument(skip(state))]
async fn interrupt_session(State(state): State<AppState>, Path(id): Path<SessionId>) -> Response {
    if let Some(entry) = state.sessions.read().await.get(&id).cloned() {
        debug!(session_id = %entry.handle.id(), "Interrupt requested");
        entry
            .handle
            .send_command(SessionCommand::Interrupt)
            .expect("closed");
    } else if !state.disk_sessions.read().await.contains_key(&id) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "status": "not_found" })),
        )
            .into_response();
    }
    Json(json!({ "status": "ok" })).into_response()
}

/// Get the trace of a running or stored session as NDJSON records.
#[tracing::instrument(skip(state), fields(session_id))]
async fn snapshot(State(state): State<AppState>, Path(id): Path<SessionId>) -> Response {
    info!("Getting snapshot");
    let body = if let Some(entry) = state.sessions.read().await.get(&id) {
        trace_store::ndjson(&entry.records.lock().unwrap_or_else(|e| e.into_inner()))
    } else if let Some(disk) = state.disk_sessions.read().await.get(&id) {
        trace_store::ndjson(&disk.records)
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    ([(CONTENT_TYPE, "application/x-ndjson")], body).into_response()
}

/// Get a stream of events for a session, including LLM responses and user
/// events.
#[tracing::instrument(skip(state), fields(session_id))]
async fn event_stream(State(state): State<AppState>, Path(id): Path<SessionId>) -> Response {
    info!("Opening event stream");
    let event_tx = if let Some(events) = state
        .sessions
        .read()
        .await
        .get(&id)
        .map(|entry| entry.events.clone())
    {
        Some(events)
    } else {
        state
            .disk_sessions
            .read()
            .await
            .get(&id)
            .map(|entry| entry.events.clone())
    };
    let Some(event_tx) = event_tx else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let stream = BroadcastStream::new(event_tx.subscribe())
        .scan(BlockTracker::default(), |tracker, result| {
            let events = result.map(|event| tracker.translate(event)).unwrap_or_default();
            futures::future::ready(Some(futures::stream::iter(events)))
        })
        .flatten()
        .map(|event| {
            Ok::<_, std::convert::Infallible>(
                axum::response::sse::Event::default().data(event.to_string()),
            )
        });
    Sse::new(stream).into_response()
}
