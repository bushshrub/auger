use crate::AutoApprovalPolicies;
use crate::SessionEvent;
use crate::SessionHandle;
use crate::SessionId;
use crate::SystemPrompt;
use crate::session::SessionRecord;
use crate::session::history::EventRecord;
use crate::session::history::ModelInfo;
use crate::session::history::SessionData;
use crate::session::history::TurnId;
use crate::session::history::TurnRecord;
use crate::session::recorder::EventCallback;
use crate::session::recorder::SessionRecorder;
use crate::session::recorder::TurnCallback;
use crate::session::runtime::Session;
use agent_tools::Tool;
use provider::LlmModel;
use std::env::current_dir;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use tokio::runtime::Handle;

pub struct SessionBuilder {
    record: SessionRecord,
    on_turn: TurnCallback,
    on_event: EventCallback,
}

impl SessionBuilder {
    pub fn new(model_name: String) -> Self {
        let id = SessionId::new();
        let record = SessionRecord::new(
            id,
            current_dir().expect("no cwd"),
            ModelInfo::new("to-be-added".to_string(), model_name),
        );
        Self {
            record,
            on_turn: Arc::new(|_, _| {}),
            on_event: Arc::new(|_, _| {}),
        }
    }

    pub fn restore(record: SessionRecord) -> Self {
        Self {
            record,
            on_turn: Arc::new(|_, _| {}),
            on_event: Arc::new(|_, _| {}),
        }
    }

    pub fn id(&self) -> SessionId {
        self.record.data().session_id()
    }

    pub fn session_data(&self) -> &SessionData {
        self.record.data()
    }

    pub fn on_turn(mut self, cb: impl Fn(TurnId, &TurnRecord) + Send + Sync + 'static) -> Self {
        self.on_turn = Arc::new(cb);
        self
    }

    pub fn on_event(mut self, cb: impl Fn(TurnId, &EventRecord) + Send + Sync + 'static) -> Self {
        self.on_event = Arc::new(cb);
        self
    }

    pub fn start(
        self,
        model: LlmModel,
        system_prompt: SystemPrompt,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        auto_approval_policies: impl Into<AutoApprovalPolicies>,
    ) -> (SessionHandle, Receiver<SessionEvent>) {
        let recorder = SessionRecorder::new(self.record, self.on_turn, self.on_event);
        Session::spawn(
            rt,
            system_prompt,
            recorder,
            model,
            tools,
            auto_approval_policies.into(),
        )
    }
}
