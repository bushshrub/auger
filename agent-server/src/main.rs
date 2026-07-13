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
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, info};
use uuid::Uuid;

use crate::server_types::{
    ApproveRequest, CreateSessionRequest, SessionEntry, SnapshotMessage, UserInputRequest,
};
use agent_core::{Session, SessionEvent, SessionHandle, SystemPrompt};
use provider::LlmModel;
use provider_openai_responses::OpenAiResponsesProvider;

mod server_types;

const DEFAULT_MODEL: &str = "qwen3.6-35b-q8";
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
    // TODO: support multiple providers
    provider: Arc<OpenAiResponsesProvider>,
    sessions: Arc<RwLock<HashMap<Uuid, SessionEntry>>>,
    default_model: String,
}

async fn write_auth(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match request.headers().get("Authorization") {
        Some(v) => v
            .to_str()
            .unwrap_or_default()
            .strip_prefix("Bearer ")
            .unwrap_or_default()
            .to_owned(),
        None => return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response(),
    };
    let entry = match state.sessions.read().await.get(&id).cloned() {
        Some(e) => e,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    if token != entry.write_token.to_string() {
        return (StatusCode::UNAUTHORIZED, "Invalid write token").into_response();
    }
    request.extensions_mut().insert(entry.handle);
    next.run(request).await
}

async fn read_auth(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = match request.headers().get("Authorization") {
        Some(v) => v
            .to_str()
            .unwrap_or_default()
            .strip_prefix("Bearer ")
            .unwrap_or_default()
            .to_owned(),
        None => return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response(),
    };
    let entry = match state.sessions.read().await.get(&id).cloned() {
        Some(e) => e,
        None => return (StatusCode::NOT_FOUND, "Session not found").into_response(),
    };
    if token != entry.read_token.to_string() && token != entry.write_token.to_string() {
        return (StatusCode::UNAUTHORIZED, "Invalid read token").into_response();
    }
    request.extensions_mut().insert(entry.handle);
    request.extensions_mut().insert(entry.events);
    next.run(request).await
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let base_url = std::env::var("PROVIDER_BASE_URL")
        .unwrap_or_else(|_| "http://server-slop:8080/v1/".to_string());
    let api_key = std::env::var("PROVIDER_API_KEY").unwrap_or_default();

    let provider = Arc::new(OpenAiResponsesProvider::new(api_key, base_url));

    let state = AppState {
        provider: Arc::clone(&provider),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        default_model: std::env::var("MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
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
        .route_layer(middleware::from_fn_with_state(state.clone(), write_auth));

    let read_routes = Router::new()
        .route("/sessions/{id}/events", get(event_stream))
        .route("/sessions/{id}/snapshot", get(snapshot))
        .route_layer(middleware::from_fn_with_state(state.clone(), read_auth));

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

/// List all active sessions
async fn list_sessions(State(state): State<AppState>) -> impl IntoResponse {
    let sessions = state.sessions.read().await;
    let list: Vec<_> = sessions
        .values()
        .map(|e| {
            json!({
                "session_id": e.handle.id().as_uuid(),
                "model": e.model,
                "created_at": e.created_at,
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
    let (owner, handle, event_receiver) = Session::start_with_tools(
        LlmModel::new(state.provider.clone(), &model),
        sys_prompt,
        tokio::runtime::Handle::current(),
        tools,
        Vec::new(),
    );
    let session_id = handle.id().as_uuid();
    let (event_tx, _) = tokio::sync::broadcast::channel(256);
    let forward_tx = event_tx.clone();
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = event_receiver.recv_event() {
            let closed = matches!(event, SessionEvent::Closed);
            let _ = forward_tx.send(event);
            if closed {
                break;
            }
        }
    });
    let read_token = Uuid::new_v4();
    let write_token = Uuid::new_v4();
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    state.sessions.write().await.insert(
        session_id,
        SessionEntry {
            handle,
            owner: Arc::new(std::sync::Mutex::new(Some(owner))),
            events: event_tx,
            model,
            created_at,
            read_token,
            write_token,
        },
    );
    Json(
        json!({ "session_id": session_id, "context_window": DEFAULT_CONTEXT_WINDOW, "tokens": {
        "read": read_token,
        "write": write_token
    } }),
    )
}

async fn stop_session(State(state): State<AppState>, Path(id): Path<Uuid>) -> impl IntoResponse {
    let Some(entry) = state.sessions.write().await.remove(&id) else {
        return StatusCode::NOT_FOUND;
    };
    if let Some(owner) = entry.owner.lock().unwrap().take() {
        owner.stop();
    }
    StatusCode::NO_CONTENT
}

/// Submit input to the clanker
#[tracing::instrument(skip(session_handle, req))]
async fn send_input(
    Extension(session_handle): Extension<SessionHandle>,
    Json(req): Json<UserInputRequest>,
) -> impl IntoResponse {
    debug!("Enqueued message: '{}'", req.input);
    session_handle.send_message(req.into()).expect("closed");

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
    session_handle
        .decide_tool_call(req.tool_call_id, req.approved, req.message)
        .expect("closed");
    // TODO: garbage return type
    Json(json!({ "status": "ok" }))
}

/// Get a point-in-time snapshot of the session's conversation history.
async fn snapshot(Extension(session_handle): Extension<SessionHandle>) -> impl IntoResponse {
    match tokio::task::spawn_blocking(move || session_handle.snapshot()).await {
        Ok(Ok(snapshot)) => {
            let snapshot: Vec<SnapshotMessage> = snapshot
                .messages()
                .iter()
                .cloned()
                .flat_map(SnapshotMessage::from_provider)
                .collect();
            Json(json!({ "messages": snapshot })).into_response()
        }
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
        SessionEvent::Closed => json!({ "type": "closed" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        AppState {
            provider: Arc::new(OpenAiResponsesProvider::new(
                "test".to_string(),
                "http://127.0.0.1:1/v1/".to_string(),
            )),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_model: "test-model".to_string(),
        }
    }

    #[tokio::test]
    async fn openapi_document_is_served() {
        let response = router(test_state())
            .oneshot(Request::get("/openapi.yaml").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(std::str::from_utf8(&body).unwrap().contains("openapi: 3.1.0"));
    }

    #[tokio::test]
    async fn create_snapshot_and_delete_session() {
        let app = router(test_state());
        let response = app
            .clone()
            .oneshot(
                Request::post("/sessions")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let id = created["session_id"].as_str().unwrap();
        let read_token = created["tokens"]["read"].as_str().unwrap();
        let write_token = created["tokens"]["write"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/sessions/{id}/snapshot"))
                    .header(header::AUTHORIZATION, format!("Bearer {read_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(snapshot["messages"], json!([]));

        let response = app
            .oneshot(
                Request::delete(format!("/sessions/{id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {write_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
}
