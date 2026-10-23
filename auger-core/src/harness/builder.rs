use crate::SessionEvent;
use crate::SessionHandle;
use crate::SystemPrompt;
use crate::harness::session::Session;
use crate::record::ModelInfo;
use crate::record::NoSink;
use crate::record::Record;
use crate::record::RecordSink;
use crate::record::RestoreError;
use crate::record::SessionData;
use crate::record::SessionId;
use crate::record::fold::Restored;
use crate::record::fold::fold;
use auger_toolengine::PolicyNode;
use auger_toolengine::Tool;
use chrono::Utc;
use provider::LlmModel;
use std::env::current_dir;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use tokio::runtime::Handle;

pub(crate) enum Origin {
    New {
        data: SessionData,
        system_prompt: String,
    },
    Restored(Restored),
}

pub struct SessionBuilder {
    origin: Origin,
    sink: Box<dyn RecordSink>,
}

impl SessionBuilder {
    pub fn new(model_name: String, system_prompt: SystemPrompt) -> Self {
        let data = SessionData::new(
            SessionId::new(),
            Utc::now(),
            current_dir().expect("no cwd"),
            ModelInfo::new("to-be-added".to_string(), model_name),
        );
        Self {
            origin: Origin::New {
                data,
                system_prompt: system_prompt.into(),
            },
            sink: Box::new(NoSink),
        }
    }

    /// Resume a session from its records, in order. The session keeps the
    /// system prompt it was started with.
    pub fn restore(records: impl IntoIterator<Item = Record>) -> Result<Self, RestoreError> {
        Ok(Self {
            origin: Origin::Restored(fold(records)?),
            sink: Box::new(NoSink),
        })
    }

    pub fn id(&self) -> SessionId {
        self.session_data().session_id()
    }

    pub fn session_data(&self) -> &SessionData {
        match &self.origin {
            Origin::New { data, .. } => data,
            Origin::Restored(restored) => &restored.header.data,
        }
    }

    /// Work in `cwd` instead of the process's working directory. Only a new
    /// session takes it; a restored session keeps its own.
    pub fn cwd(mut self, cwd: PathBuf) -> Self {
        if let Origin::New { data, .. } = &mut self.origin {
            *data = SessionData::new(
                data.session_id(),
                data.created_at(),
                cwd,
                data.model_info().clone(),
            );
        }
        self
    }

    /// Where records go. A restored session appends to the records it was
    /// restored from. Defaults to keeping nothing.
    pub fn sink(mut self, sink: impl RecordSink + 'static) -> Self {
        self.sink = Box::new(sink);
        self
    }

    /// Start the session. Fails only for a restored session whose turns the
    /// driver rejects.
    pub fn start(
        self,
        model: LlmModel,
        rt: Handle,
        tools: Vec<Box<dyn Tool>>,
        policy: Arc<dyn PolicyNode>,
    ) -> Result<(SessionHandle, Receiver<SessionEvent>), RestoreError> {
        Session::spawn(rt, self.origin, self.sink, model, tools, policy)
    }
}
