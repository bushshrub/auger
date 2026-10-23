//! The agent's event loop. Client messages and session events arrive on one
//! channel and are handled one at a time, so no handler blocks on a reply.

use crate::jsonrpc::{self, INVALID_PARAMS, Incoming, METHOD_NOT_FOUND, Out, RpcError};
use crate::session::Session;
use crate::storage;
use crate::updates::{self, Mapper};
use crate::Deps;
use auger_core::{SessionBuilder, SessionEvent, SystemPrompt};
use provider::LlmModel;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use tokio::runtime::Handle;

pub(crate) enum Inbound {
    Line(String),
    InputClosed,
    Session(String, SessionEvent),
}

pub(crate) struct Server {
    out: Out,
    deps: Deps,
    rt: Handle,
    tx: Sender<Inbound>,
    sessions: HashMap<String, Session>,
}

impl Server {
    pub(crate) fn new(out: Out, deps: Deps, rt: Handle, tx: Sender<Inbound>) -> Self {
        Self {
            out,
            deps,
            rt,
            tx,
            sessions: HashMap::new(),
        }
    }

    /// Handle messages until the input has closed and no prompt is running.
    pub(crate) fn serve(&mut self, rx: Receiver<Inbound>) {
        let mut input_closed = false;
        while let Ok(message) = rx.recv() {
            match message {
                Inbound::Line(line) => self.on_line(&line),
                Inbound::InputClosed => {
                    // The client is gone, so no permission answer can come.
                    input_closed = true;
                    for session in self.sessions.values_mut() {
                        session.deny_all();
                    }
                }
                Inbound::Session(id, event) => self.on_event(&id, event),
            }
            if input_closed && self.sessions.values().all(|s| !s.is_busy()) {
                break;
            }
        }
    }

    pub(crate) fn shutdown(&self) {
        for session in self.sessions.values() {
            session.stop();
        }
    }

    fn on_line(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        match jsonrpc::parse(line) {
            Incoming::Request { id, method, params } => {
                match self.dispatch(&id, &method, &params) {
                    Ok(Some(result)) => self.out.send(&jsonrpc::result(&id, result)),
                    Ok(None) => {}
                    Err(error) => self.out.send(&jsonrpc::error(&id, &error)),
                }
            }
            Incoming::Notification { method, params } => {
                if method == "session/cancel" {
                    match storage::session_key(&params) {
                        Ok(key) => {
                            if let Some(session) = self.sessions.get_mut(&key) {
                                session.cancel();
                            }
                        }
                        Err(error) => tracing::warn!(error = %error.message, "bad session/cancel"),
                    }
                }
            }
            Incoming::Response { id, result } => {
                if let Some(request_id) = id.as_i64() {
                    let answered = self
                        .sessions
                        .values_mut()
                        .any(|session| session.answer_permission(request_id, &result));
                    if !answered {
                        tracing::warn!(%request_id, "reply to an unknown request");
                    }
                }
            }
            Incoming::Invalid { id, error } => self.out.send(&jsonrpc::error(&id, &error)),
        }
    }

    /// `Ok(None)` means the reply comes later, when the prompt settles.
    fn dispatch(&mut self, id: &Value, method: &str, params: &Value) -> Result<Option<Value>, RpcError> {
        match method {
            "initialize" => Ok(Some(initialize())),
            "session/new" => self.new_session(params).map(Some),
            "session/load" => self.load_session(params).map(Some),
            "session/prompt" => {
                let key = storage::session_key(params)?;
                let text = prompt_text(params)?;
                let session = self
                    .sessions
                    .get_mut(&key)
                    .ok_or_else(|| RpcError::invalid(format!("unknown session {key}")))?;
                session.start_prompt(id.clone(), text).map(|()| None)
            }
            _ => Err(RpcError::new(METHOD_NOT_FOUND, format!("unknown method {method}"))),
        }
    }

    fn new_session(&mut self, params: &Value) -> Result<Value, RpcError> {
        let cwd = cwd_param(params)?;
        let prompt = format!(
            "{}\nThe current working directory is: {}",
            self.deps.system_prompt,
            cwd.display()
        );
        let builder = SessionBuilder::new(self.deps.model.clone(), SystemPrompt::new(prompt))
            .cwd(cwd.clone());
        let builder = storage::attach_sink(builder, self.deps.sessions_dir.as_deref());
        let id = self.start(builder, Mapper::new(cwd))?;
        Ok(json!({"sessionId": id}))
    }

    /// Replay the stored history as updates, then resume the session if it is
    /// not already live in this process.
    fn load_session(&mut self, params: &Value) -> Result<Value, RpcError> {
        let key = storage::session_key(params)?;
        let dir: PathBuf = self
            .deps
            .sessions_dir
            .clone()
            .ok_or_else(|| RpcError::invalid("sessions are not stored; nothing to load"))?;
        let records = storage::read(&dir, &key)?;
        let mut mapper = Mapper::new(storage::header_cwd(&records)?);
        for record in &records {
            for update in mapper.record(&record.body, true) {
                updates::send(&self.out, &key, update);
            }
        }
        if !self.sessions.contains_key(&key) {
            let builder = SessionBuilder::restore(records)
                .map_err(|error| RpcError::internal(error.to_string()))?;
            let builder = storage::attach_sink(builder, Some(&dir));
            self.start(builder, mapper)?;
        }
        Ok(json!({}))
    }

    fn start(&mut self, builder: SessionBuilder, mapper: Mapper) -> Result<String, RpcError> {
        let id = builder.id().to_string();
        let model = LlmModel::new(Arc::clone(&self.deps.provider), &self.deps.model);
        let (handle, events) = builder
            .start(
                model,
                self.rt.clone(),
                tools(),
                Arc::new(auger_policy::default_policy()),
            )
            .map_err(|error| RpcError::internal(error.to_string()))?;
        let tx = self.tx.clone();
        let forward_id = id.clone();
        std::thread::spawn(move || {
            while let Ok(event) = events.recv() {
                let closed = matches!(event, SessionEvent::Closed);
                if tx.send(Inbound::Session(forward_id.clone(), event)).is_err() || closed {
                    break;
                }
            }
        });
        self.sessions
            .insert(id.clone(), Session::new(id.clone(), handle, mapper));
        Ok(id)
    }

    fn on_event(&mut self, id: &str, event: SessionEvent) {
        let closed = matches!(event, SessionEvent::Closed);
        if let Some(session) = self.sessions.get_mut(id) {
            session.on_event(event, &self.out);
        }
        if closed {
            self.sessions.remove(id);
        }
    }
}

fn initialize() -> Value {
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {"loadSession": true},
        "authMethods": [],
        "agentInfo": {"name": "auger-acp", "version": env!("CARGO_PKG_VERSION")},
    })
}

fn cwd_param(params: &Value) -> Result<PathBuf, RpcError> {
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cwd is required"))?;
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err(RpcError::invalid("cwd must be an absolute path"));
    }
    Ok(path)
}

/// The text of a prompt. Other content blocks are not advertised, so they are dropped.
fn prompt_text(params: &Value) -> Result<String, RpcError> {
    let blocks = params
        .get("prompt")
        .and_then(Value::as_array)
        .ok_or_else(|| RpcError::invalid("prompt must be an array of content blocks"))?;
    let text: Vec<&str> = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    if text.is_empty() {
        return Err(RpcError::invalid("prompt has no text"));
    }
    Ok(text.join("\n"))
}

fn tools() -> Vec<Box<dyn auger_toolengine::Tool>> {
    vec![
        Box::new(builtin_tools::ReadFile),
        Box::new(builtin_tools::WriteFile),
        Box::new(builtin_tools::EditFile),
        Box::new(builtin_tools::Shell),
    ]
}
