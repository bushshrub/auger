mod agent;
mod session;

use std::{collections::HashMap, convert::Infallible, sync::Arc};

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Json,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use agent_tools::{ReadFile, Tool};
use futures::stream;
use provider::Provider;
use provider_openai_chatcompletions::OpenAiChatCompletionsProvider;
use session::{
    ApproveRequest, CreateSessionRequest, CreateSessionResponse, Session,
    SessionStatus, UserInputRequest,
};
use tokio::{net::TcpListener, sync::RwLock};
use uuid::Uuid;

const DEFAULT_MODEL: &str = "gemma4-12b";
const SYSTEM_PROMPT: &str = "You are a coding agent. Use tools to read, write, and run code.";

#[derive(Clone)]
struct AppState {
    provider: Arc<dyn Provider + Send + Sync>,
    sessions: Arc<RwLock<HashMap<Uuid, Arc<Session>>>>,
    tools: Arc<Vec<Arc<dyn Tool>>>,
    system_prompt: Arc<String>,
    default_model: String,
}

#[tokio::main]
async fn main() {
    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let base_url = std::env::var("PROVIDER_BASE_URL")
        .unwrap_or_else(|_| "http://server-slop:8081/v1".to_string());
    let api_key = std::env::var("PROVIDER_API_KEY").unwrap_or_default();

    let provider: Arc<dyn Provider + Send + Sync> = Arc::new(
        OpenAiChatCompletionsProvider::with_config(&base_url, &api_key),
    );

    let state = AppState {
        provider,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        tools: Arc::new(vec![Arc::new(ReadFile)]),
        system_prompt: Arc::new(SYSTEM_PROMPT.to_string()),
        default_model: std::env::var("MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
    };

    let listener = TcpListener::bind(&addr).await.expect("bind failed");
    eprintln!("agent-server listening on {addr}");
    axum::serve(listener, router(state)).await.expect("server error");
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/sessions", post(create_session))
        .route("/v1/sessions/{id}/input", post(send_input))
        .route("/v1/sessions/{id}/approve", post(approve_tool))
        .route("/v1/sessions/{id}/events", get(event_stream))
        .with_state(state)
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let session = Session::new(Uuid::new_v4(), model);
    let resp = CreateSessionResponse {
        session_id: session.id,
        owner_token: session.owner_token.clone(),
        viewer_token: session.viewer_token.clone(),
    };
    state.sessions.write().await.insert(session.id, session);
    Json(resp)
}

async fn send_input(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<UserInputRequest>,
) -> impl IntoResponse {
    let session = get_session(&state, id).await?;

    require_owner(&session, &headers)?;

    {
        let status = session.status.lock().await;
        if *status != SessionStatus::Idle {
            return Err((StatusCode::CONFLICT, "session is not idle"));
        }
    }

    session.history.lock().await.push(provider::Message {
        role: provider::Role::User,
        content: req.content,
        tool_calls: None,
        tool_call_id: None,
    });

    let session_arc = Arc::clone(&session);
    let provider = Arc::clone(&state.provider);
    let tools = Arc::clone(&state.tools);
    let system_prompt = Arc::clone(&state.system_prompt);
    tokio::spawn(async move {
        agent::run(session_arc, provider, tools, system_prompt).await;
    });

    Ok(StatusCode::ACCEPTED)
}

async fn approve_tool(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<ApproveRequest>,
) -> impl IntoResponse {
    let session = get_session(&state, id).await?;

    require_owner(&session, &headers)?;

    let pending = session.pending_approval.lock().await.take();
    match pending {
        None => Err((StatusCode::CONFLICT, "no tool call pending approval")),
        Some(p) if p.tool_call_id != req.tool_call_id => {
            // put it back — wrong id
            *session.pending_approval.lock().await = Some(p);
            Err((StatusCode::UNPROCESSABLE_ENTITY, "tool_call_id mismatch"))
        }
        Some(p) => {
            let _ = p.tx.send(req.approved);
            Ok(StatusCode::OK)
        }
    }
}

async fn event_stream(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let session = get_session(&state, id).await.map_err(IntoResponse::into_response)?;

    let token = bearer_token(&headers).unwrap_or_default();
    if !session.can_view(token) {
        return Err((StatusCode::UNAUTHORIZED, "invalid token").into_response());
    }

    let rx = session.events.subscribe();
    let sse = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some((Ok::<_, Infallible>(Event::default().data(data)), rx))
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                let data = format!(r#"{{"type":"lagged","missed":{n}}}"#);
                Some((Ok(Event::default().data(data)), rx))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
        }
    });

    Ok(Sse::new(sse).keep_alive(KeepAlive::default()).into_response())
}

// --- helpers ---

async fn get_session(
    state: &AppState,
    id: Uuid,
) -> Result<Arc<Session>, (StatusCode, &'static str)> {
    state
        .sessions
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or((StatusCode::NOT_FOUND, "session not found"))
}

fn require_owner(
    session: &Session,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, &'static str)> {
    match bearer_token(headers) {
        Some(t) if session.is_owner(t) => Ok(()),
        _ => Err((StatusCode::UNAUTHORIZED, "owner token required")),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}
