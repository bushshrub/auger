use std::collections::HashMap;
use std::sync::Arc;
use axum::extract::{Path, State};
use axum::{Json, Router};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Sse};
use axum::routing::{get, post};
use futures::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;
use agent_tools::{ReadFile, Tool};
use provider::LlmProvider;
use provider_openai_chatcompletions::OpenAiChatCompletionsProvider;
use provider_openai_responses::OpenAiResponsesProvider;
use session::handle::SessionHandle;
use crate::server_types::{ApproveRequest, CreateSessionRequest, UserInputRequest};
use crate::session::Session;
use crate::system_prompt::SystemPrompt;

mod session;
mod conversation;
mod system_prompt;
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
    Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}/input", post(send_input))
        .route("/sessions/{id}/events", get(event_stream))
        .route("/sessions/{id}/tool", post(response_to_tool_call))
        .with_state(state)
}

/// Request to create a session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let model = req.model.unwrap_or_else(|| DEFAULT_MODEL.into());
    let sys_prompt = SystemPrompt::new(SYSTEM_PROMPT.to_string());
    let session_handle = Session::spawn(sys_prompt, &state.provider, model);
    let session_id = session_handle.id;
    let read_token = session_handle.read_token;
    let write_token = session_handle.write_token;
    state.sessions.write().await.insert(session_id, session_handle);
    Json(json!({ "session_id": session_id, "tokens": {
        "read": read_token,
        "write": write_token
    } }))
}

/// Submit input to the clanker
#[tracing::instrument(skip(state, headers, req))]
async fn send_input(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<UserInputRequest>,
) -> impl IntoResponse {
    let session_handle = {
        let sessions = state.sessions.read().await;
        match sessions.get(&id) {
            Some(handle) => {
                debug!("Session found");
                handle.clone()
            },
            None => return Err((StatusCode::NOT_FOUND, "Session not found").into_response())
        }
    };
    let write_token = match headers.get("Authorization") {
        Some(token) => token.to_str().unwrap().strip_prefix("Bearer ").unwrap_or_default(),
        None => return Err((StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response())
    };
    if write_token != session_handle.write_token.to_string() {
        return Err((StatusCode::UNAUTHORIZED, "Invalid write token").into_response());
    }
    debug!("Enqueued message: '{}'", req.input);
    session_handle.enqueue(vec![req.input.into()]).await.expect("closed");

    // TODO: bad return type
    Ok(Json(json!({ "status": "ok" })))
}

/// Response to a tool call
async fn response_to_tool_call(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<ApproveRequest>,
) -> impl IntoResponse {
    let session_handle = {
        let sessions = state.sessions.read().await;
        debug!("Looking up session {id}");
        match sessions.get(&id) {
            Some(handle) => handle.clone(),
            None => return Err((StatusCode::NOT_FOUND, "Session not found").into_response())
        }
    };
    let write_token = match headers.get("Authorization") {
        Some(token) => token.to_str().unwrap().strip_prefix("Bearer ").unwrap_or_default(),
        None => return Err((StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response())
    };
    if write_token != session_handle.write_token.to_string() {
        return Err((StatusCode::UNAUTHORIZED, "Invalid write token").into_response());
    }
    debug!(session_id = %id, call_id = %req.tool_call_id, "Approving tool");
    session_handle.respond_to_tool_call(req.tool_call_id, req.approved).await.expect("closed");
    // TODO: garbage return type
    Ok(Json(json!({ "status": "ok" })))
}

/// Get a stream of events for a session, including LLM responses and user events.
#[tracing::instrument(skip(state, headers))]
async fn event_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session_handle = {
        let sessions = state.sessions.read().await;
        debug!("Looking up session {id}");
        match sessions.get(&id) {
            Some(handle) => handle.clone(),
            None => return Err((StatusCode::NOT_FOUND, "Session not found").into_response())
        }
    };
    let read_token = match headers.get("Authorization") {
        Some(token) => token.to_str().unwrap().strip_prefix("Bearer ").unwrap_or_default(),
        None => return Err((StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response())
    };
    let sess_read_token = session_handle.read_token.to_string();
    let sess_write_token = session_handle.write_token.to_string();
    if read_token != sess_read_token && read_token != sess_write_token {
        return Err((StatusCode::UNAUTHORIZED, "Invalid read token").into_response());
    }
    let stream = BroadcastStream::new(session_handle.subscribe());
    Ok(Sse::new(stream.filter_map(|result| async move {
        let event = result.ok()?;
        let json = serde_json::to_string(&event).ok()?;
        Some(Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(json)))
    })))
}
