use std::collections::HashMap;
use std::sync::Arc;
use axum::extract::{Path, State};
use axum::{Json, Router};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use uuid::Uuid;
use agent_tools::{ReadFile, Tool};
use provider::LlmProvider;
use provider_openai_chatcompletions::OpenAiChatCompletionsProvider;
use crate::server_types::{ApproveRequest, CreateSessionRequest, UserInputRequest};
use crate::session::{Session, SessionHandle};


mod session;
mod conversation;
mod system_prompt;
mod server_types;

const DEFAULT_MODEL: &str = "gemma4-12b";
const DEFAULT_CONTEXT_WINDOW: usize = 8192;
const SYSTEM_PROMPT: &str = "You are a coding agent. Use tools to read, write, and run code.";

#[derive(Clone)]
struct AppState {
    // TODO: support multiple providers
    provider: Arc<OpenAiChatCompletionsProvider>,
    sessions: Arc<RwLock<HashMap<Uuid, SessionHandle>>>,
    tools: Arc<Vec<Arc<dyn Tool>>>,
    default_model: String
}

#[tokio::main]
async fn main() {
    let addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let base_url = std::env::var("PROVIDER_BASE_URL")
        .unwrap_or_else(|_| "http://server-slop:8081/v1".to_string());
    let api_key = std::env::var("PROVIDER_API_KEY").unwrap_or_default();

    let provider = Arc::new(OpenAiChatCompletionsProvider::new(api_key, base_url));

    let state = AppState {
        provider: Arc::clone(&provider),
        sessions: Arc::new(RwLock::new(HashMap::new())),
        tools: Arc::new(vec![Arc::new(ReadFile)]),
        default_model: std::env::var("MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
    };

    let listener = TcpListener::bind(&addr).await.expect("bind failed");
    eprintln!("agent-server listening on {addr}");
    axum::serve(listener, router(state)).await.expect("server error");
}


fn router(state: AppState) -> Router {
    Router::new()
        // .route("/v1/sessions", post(create_session))
        // .route("/v1/sessions/{id}/input", post(send_input))
        // .route("/v1/sessions/{id}/approve", post(approve_tool))
        // .route("/v1/sessions/{id}/events", get(event_stream))
        .with_state(state)
}

// /// Request to create a session
// async fn create_session(
//     State(state): State<AppState>,
//     Json(req): Json<CreateSessionRequest>,
// ) -> impl IntoResponse {
//     req.model.unwrap_or_else(|| DEFAULT_MODEL.into());
//     // TODO: customizable system prompt
//     let sys_prompt = SYSTEM_PROMPT.to_string();
//     todo!();

//     let sess_id = sess.id();
//     state.sessions.write().await.insert(sess_id, sess);
//     // TODO: hardcoded json
//     // TODO: return read, write tokens
//     Json(json!({ "session_id": sess_id }))
// }

// /// Submit input to the clanker
// async fn send_input(
//     State(state): State<AppState>,
//     Path(id): Path<Uuid>,
//     headers: HeaderMap,
//     Json(req): Json<UserInputRequest>,
// ) -> impl IntoResponse {
//     let mut sess = state.sessions.read().await.get(&id).expect("session not found todo: better err handling").clone();
//     // sess.send_message(req.input.into()).await.expect("failed to send input");

//     // TODO: bad return type
//     Ok(Json(json!({ "message": sess })))
// }

// /// Approve the usage of a tool
// async fn approve_tool(
//     State(state): State<AppState>,
//     Path(id): Path<Uuid>,
//     headers: HeaderMap,
//     Json(req): Json<ApproveRequest>,
// ) -> impl IntoResponse {
//     todo!()
// }

// /// Get a stream of events for a session, including LLM responses and user events.
// async fn event_stream(
//     State(state): State<AppState>,
//     Path(id): Path<Uuid>,
//     headers: HeaderMap,
// ) -> impl IntoResponse {
//     todo!()
// }
