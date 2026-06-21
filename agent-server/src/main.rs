use std::collections::HashMap;
use std::sync::Arc;
use axum::extract::{Path, Request, State};
use axum::{middleware, Json, Router};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::Extension;
use futures::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;
use agent_tools::{ReadFile, Tool};
use provider_openai_responses::OpenAiResponsesProvider;
use agent_core::{Session, SessionHandle, SystemPrompt};
use crate::server_types::{ApproveRequest, CreateSessionRequest, UserInputRequest};

mod server_types;

const DEFAULT_MODEL: &str = "qwen3.6-27b";
const DEFAULT_CONTEXT_WINDOW: usize = 8192;
const SYSTEM_PROMPT: &str = "You are a coding agent. Use tools to read, write, and run code.";

#[derive(Clone)]
struct AppState {
    // TODO: support multiple providers
    provider: Arc<OpenAiResponsesProvider>,
    sessions: Arc<RwLock<HashMap<Uuid, SessionHandle>>>,
    tools: Arc<Vec<Arc<dyn Tool>>>,
    default_model: String
}

async fn write_auth(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match request.headers().get("Authorization") {
        Some(v) => v.to_str().unwrap_or_default().strip_prefix("Bearer ").unwrap_or_default().to_owned(),
        None => return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response(),
    };
    let handle = match state.sessions.read().await.get(&id).cloned() {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    if token != handle.write_token.to_string() {
        return (StatusCode::UNAUTHORIZED, "Invalid write token").into_response();
    }
    request.extensions_mut().insert(handle);
    next.run(request).await
}

async fn read_auth(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match request.headers().get("Authorization") {
        Some(v) => v.to_str().unwrap_or_default().strip_prefix("Bearer ").unwrap_or_default().to_owned(),
        None => return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response(),
    };
    let handle = match state.sessions.read().await.get(&id).cloned() {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    if token != handle.read_token.to_string() && token != handle.write_token.to_string() {
        return (StatusCode::UNAUTHORIZED, "Invalid read token").into_response();
    }
    request.extensions_mut().insert(handle);
    next.run(request).await
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let base_url = std::env::var("PROVIDER_BASE_URL")
        .unwrap_or_else(|_| "http://server-slop:8080/v1".to_string());
    let api_key = std::env::var("PROVIDER_API_KEY").unwrap_or_default();

    let provider = Arc::new(OpenAiResponsesProvider::new(api_key, base_url));

    let state = AppState {
        provider: Arc::clone(&provider),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        tools: Arc::new(vec![Arc::new(ReadFile)]),
        default_model: std::env::var("MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
    };

    let listener = TcpListener::bind(&addr).await.expect("bind failed");
    info!("agent-server listening on {addr}");
    axum::serve(listener, router(state)).await.expect("server error");
}


fn router(state: AppState) -> Router {
    let write_routes = Router::new()
        .route("/sessions/{id}/input", post(send_input))
        .route("/sessions/{id}/tool", post(respond_to_tool_call))
        .route_layer(middleware::from_fn_with_state(state.clone(), write_auth));

    let read_routes = Router::new()
        .route("/sessions/{id}/events", get(event_stream))
        .route_layer(middleware::from_fn_with_state(state.clone(), read_auth));

    Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .merge(write_routes)
        .merge(read_routes)
        .with_state(state)
}

/// List all active sessions
async fn list_sessions(State(state): State<AppState>) -> impl IntoResponse {
    let sessions = state.sessions.read().await;
    let list: Vec<_> = sessions.values().map(|h| json!({
        "session_id": h.id,
        "model": h.model,
        "created_at": h.created_at,
        "context_window": DEFAULT_CONTEXT_WINDOW,
        "tokens": {
            "read": h.read_token.to_string(),
            "write": h.write_token.to_string(),
        }
    })).collect();
    Json(json!({ "sessions": list }))
}

/// Request to create a session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let sys_prompt = SystemPrompt::new(SYSTEM_PROMPT.to_string());
    let session_handle = Session::spawn(sys_prompt, &state.provider, model);
    let session_id = session_handle.id;
    let read_token = session_handle.read_token;
    let write_token = session_handle.write_token;
    state.sessions.write().await.insert(session_id, session_handle);
    Json(json!({ "session_id": session_id, "context_window": DEFAULT_CONTEXT_WINDOW, "tokens": {
        "read": read_token,
        "write": write_token
    } }))
}

/// Submit input to the clanker
#[tracing::instrument(skip(session_handle, req))]
async fn send_input(
    Extension(session_handle): Extension<SessionHandle>,
    Json(req): Json<UserInputRequest>,
) -> impl IntoResponse {
    debug!("Enqueued message: '{}'", req.input);
    session_handle.enqueue(vec![req.input.into()]).expect("closed");

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
    span.record("session_id", tracing::field::display(&session_handle.id));
    span.record("call_id", tracing::field::display(&req.tool_call_id));
    debug!("Tool call approved: {}", req.approved);
    session_handle.respond_to_tool_call(req.tool_call_id, req.approved).expect("closed");
    // TODO: garbage return type
    Json(json!({ "status": "ok" }))
}

/// Get a stream of events for a session, including LLM responses and user events.
#[tracing::instrument(skip(session_handle))]
async fn event_stream(
    Extension(session_handle): Extension<SessionHandle>,
) -> impl IntoResponse {
    let stream = BroadcastStream::new(session_handle.subscribe());
    Sse::new(stream.filter_map(|result| async move {
        let event = result.ok()?;
        let json = serde_json::to_string(&event).ok()?;
        Some(Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(json)))
    }))
}
