use crate::session::history::{EventRecord, ModelInfo, TurnId, TurnRecord};
use crate::session::recorder::{EventCallback, SessionRecorder, TurnCallback};
use crate::session::SessionRecord;
use crate::{AutoApprovalPolicies, SessionEvent, SessionHandle, SessionId, SystemPrompt};
use agent_tools::Tool;
use provider::LlmModel;
use std::env::current_dir;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use tokio::runtime::Handle;
use crate::session::session::Session;

pub struct SessionBuilder {
    record: SessionRecord,
    on_turn: TurnCallback,
    on_event: EventCallback,
}

impl SessionBuilder {
    pub fn new(model_name: String) -> Self {
        let id = SessionId::new();
        let record = SessionRecord::new(id, current_dir().expect("no cwd"), ModelInfo::new("to-be-added".to_string(), model_name));
        Self {
            record,
            on_turn: Arc::new(|_, _| {}),
            on_event: Arc::new(|_, _| {}),
        }
    }

    pub fn id(&self) -> SessionId {
        self.record.session_id()
    }

    pub fn root_turn_id(&self) -> crate::session::history::TurnId {
        self.record.root_id()
    }

    pub fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.record.created_at()
    }

    pub fn on_turn(mut self, cb: impl Fn(TurnId, &TurnRecord) + Send + Sync + 'static) -> Self {
        self.on_turn = Arc::new(cb);
        self
    }

    pub fn on_event(mut self, cb: impl Fn(TurnId, &EventRecord) + Send + Sync + 'static) -> Self {
        self.on_event = Arc::new(cb);
        self
    }

    pub fn start(self, model: LlmModel, system_prompt: SystemPrompt, rt: Handle, tools: Vec<Box<dyn Tool>>, auto_approval_policies: impl Into<AutoApprovalPolicies>) -> (SessionHandle, Receiver<SessionEvent>) {
        let recorder = SessionRecorder::new(self.record, self.on_turn, self.on_event);
        Session::spawn(rt, system_prompt, recorder, model, tools, auto_approval_policies.into())
    }

}
