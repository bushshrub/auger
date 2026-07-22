use std::env::current_dir;
use std::sync::mpsc::Receiver;
use tokio::runtime::Handle;
use agent_tools::Tool;
use provider::LlmModel;
use crate::session::history::ModelInfo;
use crate::session::recorder::{EventCallback, TurnCallback};
use crate::session::SessionRecord;
use crate::{AutoApprovalPolicies, Session, SessionEvent, SessionHandle, SessionId, SystemPrompt};

pub struct SessionBuilder {
    record: SessionRecord,
}

impl SessionBuilder {
    pub fn new(model_name: String) -> Self {
        let id = SessionId::new();
        let record = SessionRecord::new(id, current_dir().expect("no cwd"), ModelInfo::new("to-be-added".to_string(), model_name));
        Self {
            record
        }
    }

    pub fn on_turn(self, cb: TurnCallback) -> Self {
        todo!()
    }

    pub fn on_event(self, cb: EventCallback) -> Self {
        todo!()
    }

    pub fn start(model: LlmModel, system_prompt: SystemPrompt, rt: Handle, tools: Vec<Box<dyn Tool>>, auto_approval_policies: impl Into<AutoApprovalPolicies>) -> (SessionHandle, Receiver<SessionEvent>) {
        Session::spawn(rt, system_prompt, )
    }

}