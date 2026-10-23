//! The session trace: an append-only sequence of [`Record`]s that holds
//! everything needed to restore a session, e.g. after a crash.
//!
//! Core produces records and hands each one to the embedder's [`RecordSink`];
//! where they are stored is up to the embedder. [`fold`] turns a stored
//! sequence back into what a session needs to resume.

pub(crate) mod cursor;
pub(crate) mod fold;
pub(crate) mod jsonl;
pub(crate) mod session;
pub(crate) mod writer;

use auger_driver::InputEntry;
use auger_driver::Turn;
use auger_toolengine::ToolFact;
use chrono::DateTime;
use chrono::Utc;
use provider::AssistantResponse;
use provider::LlmError;
use provider::TokenUsage;
use serde::Deserialize;
use serde::Serialize;

pub use fold::RestoreError;
pub use jsonl::JsonlSink;
pub use jsonl::read_jsonl;
pub use jsonl::ReadError;
pub use session::ModelInfo;
pub use session::SessionData;
pub use session::SessionId;

/// One line of a trace.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Record {
    /// Position in the session, starting at 0 with no gaps. A client that
    /// has seen record `n` resumes from `n + 1`.
    pub seq: u64,
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub body: RecordBody,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RecordBody {
    /// Always the first record.
    Session(SessionHeader),
    /// The driver's conversation changed.
    Turns(TurnsChange),
    /// An LLM stream started or ended.
    Stream(StreamRecord),
    /// The tool engine reported a fact.
    Tool(ToolFact),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionHeader {
    pub version: u32,
    #[serde(flatten)]
    pub data: SessionData,
    /// The prompt the session was started with. A restored session keeps
    /// using it.
    pub system_prompt: String,
}

/// How the driver's turns changed. The turns only grow at the end, apart
/// from [`TurnsChange::Truncate`], so these three cover every transition.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TurnsChange {
    PushTurn(Turn),
    /// The last turn, an input turn, gained an entry.
    PushEntry(InputEntry),
    /// Keep only the first `len` turns.
    Truncate { len: usize },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum StreamRecord {
    Started,
    /// The model's response is in the turns.
    Completed {
        usage: Option<TokenUsage>,
        stop_reason: Option<String>,
    },
    /// The user interrupted the stream. The partial is not in the turns
    /// unless a later change seals it.
    Interrupted { partial: Option<AssistantResponse> },
    Failed {
        partial: Option<AssistantResponse>,
        error: LlmError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error("failed to store record: {0}")]
pub struct SinkError(pub String);

/// Where a session's records go. Supplied by the embedder.
///
/// Records arrive in order, once each. Returning `Ok` means the record is as
/// durable as the sink promises; core waits for it before a tool runs, so a
/// sink that syncs to disk gives crash-safe tool execution.
pub trait RecordSink: Send {
    fn append(&mut self, record: &Record) -> Result<(), SinkError>;
}

/// A sink that keeps nothing.
pub struct NoSink;

impl RecordSink for NoSink {
    fn append(&mut self, _record: &Record) -> Result<(), SinkError> {
        Ok(())
    }
}
