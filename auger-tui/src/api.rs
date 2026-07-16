use futures::StreamExt;
use reqwest::Client;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::types::{
    AppEvent, RawSessionEvent, SessionInfo, SnapshotMessage, SseEvent, TuiEvent,
    transform_raw_event,
};

fn parse_sse_data(frame: &str) -> Option<String> {
    let lines: Vec<&str> = frame.lines().collect();
    let data: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("data:").map(|s| s.trim()))
        .collect();
    if data.is_empty() {
        None
    } else {
        Some(data.join("\n"))
    }
}

pub async fn list_sessions(server: &str) -> anyhow::Result<Vec<SessionInfo>> {
    let client = Client::new();
    let resp: serde_json::Value = client
        .get(format!("{server}/sessions"))
        .send()
        .await?
        .json()
        .await?;

    let sessions = resp["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| {
            let session_id = s["session_id"].as_str()?.parse::<Uuid>().ok()?;
            let model = s["model"].as_str().unwrap_or("unknown").to_string();
            let context_window = s["context_window"].as_u64().unwrap_or(8192);
            // agent-server returns tokens.read / tokens.write (not owner_token/viewer_token)
            let write_token = s["tokens"]["write"].as_str().unwrap_or("").to_string();
            let read_token = s["tokens"]["read"].as_str().unwrap_or("").to_string();
            Some(SessionInfo {
                session_id,
                model,
                context_window,
                write_token,
                read_token,
            })
        })
        .collect();

    Ok(sessions)
}

pub async fn create_session(server: &str, model: Option<&str>) -> anyhow::Result<AppEvent> {
    let client = Client::new();
    let body = json!({ "model": model });
    let resp: serde_json::Value = client
        .post(format!("{server}/sessions"))
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    let session_id = resp["session_id"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or_else(|| anyhow::anyhow!("missing session_id"))?;
    let write_token = resp["tokens"]["write"].as_str().unwrap_or("").to_string();
    let read_token = resp["tokens"]["read"].as_str().unwrap_or("").to_string();
    let context_window = resp["context_window"].as_u64().unwrap_or(8192);

    Ok(AppEvent::SessionCreated {
        session_id,
        write_token,
        read_token,
        context_window,
    })
}

pub async fn send_input(
    server: &str,
    session_id: Uuid,
    write_token: &str,
    input: &str,
) -> anyhow::Result<()> {
    let client = Client::new();
    client
        .post(format!("{server}/sessions/{session_id}/input"))
        .header("Authorization", format!("Bearer {write_token}"))
        .json(&json!({ "input": input }))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub async fn respond_to_tool(
    server: &str,
    session_id: Uuid,
    write_token: &str,
    tool_call_id: &str,
    approved: bool,
    message: Option<&str>,
) -> anyhow::Result<()> {
    let client = Client::new();
    let mut body = json!({ "tool_call_id": tool_call_id, "approved": approved });
    if let Some(msg) = message {
        body["message"] = json!(msg);
    }
    client
        .post(format!("{server}/sessions/{session_id}/tool"))
        .header("Authorization", format!("Bearer {write_token}"))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub async fn get_snapshot(
    server: &str,
    session_id: Uuid,
    token: &str,
) -> anyhow::Result<Vec<SnapshotMessage>> {
    let client = Client::new();
    let resp: serde_json::Value = client
        .get(format!("{server}/sessions/{session_id}/snapshot"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?
        .json()
        .await?;

    let messages = resp["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| serde_json::from_value(m).ok())
        .collect();

    Ok(messages)
}

/// Spawn a task that loads the snapshot then streams SSE events, both forwarded to `tx`.
pub fn spawn_event_stream(
    server: String,
    session_id: Uuid,
    token: String,
    tx: mpsc::Sender<TuiEvent>,
) {
    tokio::spawn(async move {
        // Load snapshot first so history is populated before live events arrive.
        match get_snapshot(&server, session_id, &token).await {
            Ok(msgs) => {
                let _ = tx.send(TuiEvent::App(AppEvent::SnapshotLoaded(msgs))).await;
            }
            Err(_) => {} // non-fatal: continue with empty history
        }

        let client = Client::new();
        let resp = match client
            .get(format!("{server}/sessions/{session_id}/events"))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let _ = tx
                    .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                    .await;
                return;
            }
        };

        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(bytes.as_ref()));
                    while let Some(pos) = buffer.find("\n\n") {
                        let frame = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();
                        if let Some(data) = parse_sse_data(&frame) {
                            match serde_json::from_str::<RawSessionEvent>(&data) {
                                Ok(raw) => {
                                    for ev in transform_raw_event(raw) {
                                        let _ = tx.send(TuiEvent::App(AppEvent::Sse(ev))).await;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx
                                        .send(TuiEvent::App(AppEvent::Sse(SseEvent::StreamError {
                                            message: format!("parse error: {e}"),
                                        })))
                                        .await;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(TuiEvent::App(AppEvent::NetworkError(e.to_string())))
                        .await;
                    return;
                }
            }
        }
    });
}
