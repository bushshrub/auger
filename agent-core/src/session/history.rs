use crate::tools::tool_execution::{ToolCallResult, ToolData};
use crate::SessionId;
use auger_driver::ToolCallId;
use chrono::{DateTime, Utc};
use getset::{CopyGetters, Getters};
use provider::{AssistantResponse, ToolResult, UserPrompt};
use serde::{Deserialize, Serialize};
use std::cmp::PartialEq;
use std::path::PathBuf;
use uuid::{NoContext, Timestamp, Uuid};

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
#[getset(get = "pub")]
pub struct ModelInfo {
    provider: String,
    id: String,
}

impl ModelInfo {
    pub(crate) fn new(provider: String, id: String) -> Self {
        ModelInfo { provider, id }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Getters, CopyGetters)]
pub struct SessionData {
    #[getset(get_copy = "pub")]
    session_id: SessionId,
    #[getset(get_copy = "pub")]
    created_at: DateTime<Utc>,
    #[getset(get = "pub")]
    cwd: PathBuf,
    #[getset(get = "pub")]
    model_info: ModelInfo,
}

impl SessionData {
    pub fn new(session_id: SessionId, created_at: DateTime<Utc>, cwd: PathBuf, model_info: ModelInfo) -> Self {
        Self { session_id, created_at, cwd, model_info }
    }
}

/// A record of an auger session
#[derive(Debug, Clone, Getters)]
pub struct SessionRecord {
    #[getset(get = "pub")]
    data: SessionData,
    turns: Vec<TurnRecord>,
}

impl SessionRecord {
    /// Initialize a new session record. This should be called
    /// at the start of the session.
    pub(crate) fn new(session_id: SessionId, cwd: PathBuf, model_info: ModelInfo) -> Self {
        let created_at = Utc::now();
        let turns = Vec::new();
        Self {
            data: SessionData::new(session_id, created_at, cwd, model_info),
            turns,
        }
    }

    pub(super) fn from_trace_parts(
        session_id: SessionId,
        created_at: DateTime<Utc>,
        cwd: PathBuf,
        model_info: ModelInfo,
        turns: Vec<TurnRecord>,
    ) -> Self {
        Self { data: SessionData::new(session_id, created_at, cwd, model_info), turns }
    }

    pub fn get_turn(&self, turn_id: &TurnId) -> Option<&TurnRecord> {
        self.turns.iter().find(|tr| tr.data.turn_id == *turn_id)
    }

    pub fn get_turn_mut(&mut self, turn_id: &TurnId) -> Option<&mut TurnRecord> {
        self.turns.iter_mut().find(|tr| tr.data.turn_id == *turn_id)
    }

    pub fn turns(&self) -> impl Iterator<Item = &TurnRecord> {
        self.turns.iter()
    }

    pub fn get_previous_turn(&self) -> Option<&TurnRecord> {
        // should only be None if the session JUST started.
        self.turns.last()
    }

    pub fn get_previous_turn_mut(&mut self) -> Option<&mut TurnRecord> {
        self.turns.last_mut()
    }

    pub(crate) fn add_turn(&mut self, turn: RecordableTurn) -> Result<TurnRecord, ()> {
        let previous_turn = self.turns.last();
        match previous_turn {
            Some(prev_turn) => {
                match (&turn, &prev_turn.data.turn) {
                    (RecordableTurn::InputMessage {..}, RecordableTurn::AssistantMessage {..}) | (RecordableTurn::AssistantMessage {..}, RecordableTurn::InputMessage {..}) => {
                        let tr = TurnRecord::new(turn, Some(prev_turn.data.turn_id()));
                        self.turns.push(tr.clone());
                        Ok(tr)
                    }
                    // TODO: better error information about mismatch.
                    _ => {Err(())}
                }
            },
            None => {
                // This should only happen if the session just started and this is the first turn.
                match &turn {
                    RecordableTurn::InputMessage {..} => {
                        let tr = TurnRecord::new(turn, None);
                        self.turns.push(tr.clone());
                        Ok(tr)
                    },
                    _ => Err(())
                }
            }
        }
    }

    pub fn as_messages(&self) -> Vec<provider::Message> {
        let mut messages = Vec::new();
        for turn in &self.turns {
                match turn.data.turn() {
                    RecordableTurn::InputMessage { content } => {
                        let mut msg = String::new();
                        let tool_calls = content.iter().filter_map(|c| {
                            match c {
                                InputContent::Text { text } => {
                                    msg.push_str(text.as_str());
                                    None
                                }
                                InputContent::ToolResult { tool_call_id, content } => {
                                    // TODO: Folding into a string is messy. Ideally the provider should support it natively somehow.
                                    let tool_result_content = content.iter().fold(String::new(), |mut acc, c| {
                                        match c {
                                            ToolData::Text { text } => {
                                                acc.push_str(text.as_str());
                                            }
                                        }
                                        acc
                                    });
                                    Some(ToolResult::new(tool_call_id.clone().into(), tool_result_content))
                                }
                            }
                        }).collect();
                        messages.push(provider::Message::User {
                            message: UserPrompt::new(msg),
                            tool_call_results: tool_calls
                        })
                    }
                    RecordableTurn::AssistantMessage { .. } => {
                        messages.push(turn.data.turn().clone().try_into().expect("Failed to convert turn to message"));
                    }
                }
        }
        messages
    }

}

/// ID of an event in an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(Uuid);

/// ID of a turn in an auger session. A turn is something like user/assistant etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TurnId(Uuid);

impl TurnId {
    pub(crate) fn new(time: DateTime<Utc>) -> Self {
        Self(uuid_v7_from(time))
    }
}

impl EventId {
    pub(crate) fn new(time: DateTime<Utc>) -> Self {
        Self(uuid_v7_from(time))
    }
}

impl Into<Uuid> for TurnId {
    fn into(self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for TurnId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl Into<Uuid> for EventId {
    fn into(self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for EventId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

/// A record of an event that occurred during an auger session.
/// Only events that the harness actually processed will be recorded.
#[derive(Serialize, Deserialize, Debug, Clone, CopyGetters, Getters)]
pub struct EventRecord {
    /// The logical parent of this event.
    #[getset(get_copy = "pub")]
    parent_id: Option<EventId>,
    /// Timestamp at which this event occurred.
    #[getset(get = "pub")]
    timestamp: DateTime<Utc>,
    /// Id of this event
    #[getset(get_copy = "pub")]
    event_id: EventId,
    /// The actual event itself
    #[getset(get = "pub")]
    event: RecordableEvent,
}

#[derive(Debug, Clone, Getters, CopyGetters)]
pub struct TurnEvent {
    #[getset(get_copy = "pub")]
    turn_id: TurnId,
    #[getset(get = "pub")]
    record: EventRecord,
}

impl TurnEvent {
    pub fn new(turn_id: TurnId, record: EventRecord) -> Self {
        Self { turn_id, record }
    }
}

impl EventRecord {
    fn new(parent_id: Option<EventId>, timestamp: DateTime<Utc>, event: RecordableEvent) -> Self {
        let event_id = EventId::new(timestamp);
        Self {
            parent_id,
            timestamp,
            event_id,
            event,
        }
    }

}
#[derive(Serialize, Deserialize, Debug, Clone, CopyGetters, Getters)]
pub struct TurnData {
    /// The ID of the turn.
    #[getset(get_copy = "pub")]
    turn_id: TurnId,
    #[getset(get = "pub")]
    timestamp: DateTime<Utc>,
    /// Parent of the turn
    #[getset(get_copy = "pub")]
    parent_id: Option<TurnId>,
    #[getset(get = "pub")]
    turn: RecordableTurn,
}

impl TurnData {
    fn new(turn_id: TurnId, timestamp: DateTime<Utc>, parent_id: Option<TurnId>, turn: RecordableTurn) -> Self {
        Self { turn_id, timestamp, parent_id, turn }
    }
}

// TODO: should be enum, since only assistant turns can technically have events attached to it.
#[derive(Debug, Clone, CopyGetters, Getters)]
pub struct TurnRecord {
    #[getset(get = "pub")]
    data: TurnData,
    /// The events that occurred during the turn.
    #[getset(get = "pub")]
    events: Vec<EventRecord>,
}

impl TurnRecord {
    fn new(turn: RecordableTurn, parent_id: Option<TurnId>) -> Self {
        let timestamp = Utc::now();
        let turn_id = TurnId::new(timestamp);
        let data = TurnData::new(turn_id, timestamp, parent_id, turn);
        Self {
            data,
            events: Vec::new(),
        }
    }

    pub(crate) fn from_parts(data: TurnData, events: Vec<EventRecord>) -> Self {
        Self { data, events }
    }

    pub(crate) fn restore_event(&mut self, event: EventRecord) {
        self.events.push(event);
    }

    pub(crate) fn add_event(&mut self, event: RecordableEvent, parent_id: Option<EventId>) -> Result<EventRecord, ()> {
        match &self.data.turn {
            RecordableTurn::InputMessage { .. } => {
                Err(())
            }
            RecordableTurn::AssistantMessage { outcome: status, .. } => {
                match status {
                    AssistantTurnOutcome::Completed { response: _ } => {
                        let ts = Utc::now();
                        let record = EventRecord::new(parent_id, ts, event);
                        self.events.push(record.clone());
                        Ok(record)
                    }
                    _ => {
                        Err(())
                    }
                }
            }
        }
    }

    pub(super) fn get_tool_decision_event_id(&self, tool_call_id: &ToolCallId) -> Option<EventId> {
        self.events.iter().find_map(|event| {
            let record_type = &event.event;
            match record_type {
                RecordableEvent::ToolAuthorization { tool_call_id: id, .. } if id == tool_call_id => Some(event.event_id),
                _ => None
            }
        })
    }

    pub(super) fn get_tool_call_event_id(&self, tool_call_id: &ToolCallId) -> Option<EventId> {
        self.events.iter().find_map(|event| {
            let record_type = &event.event;
            match record_type {
                RecordableEvent::ToolCallRequested { tool_call_id: id, .. } if id == tool_call_id => Some(event.event_id),
                _ => None
            }
        })
    }

    pub(crate) fn record_tool_result(&mut self, tool_result: ToolCallResult) -> Result<EventRecord, ()> {
        let tool_call_id = tool_result.tool_call_id();
        match self.get_tool_call_event_id(&tool_call_id) {
            Some(id) => self.add_event(tool_result.into(), Some(id)),
            None => Err(())
        }
    }


}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDecision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationSource {
    User,
    Policy,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContent {
    Text {
        text: String,
    },
    ToolResult {
        tool_call_id: ToolCallId,
        content: Vec<ToolData>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecordableTurn {
    /// Input message from the harness. May be due to automatic - returning tool results, or just user sending message.
    InputMessage {
        content: Vec<InputContent>
    },
    /// Result emitted by the clanker.
    AssistantMessage {
        outcome: AssistantTurnOutcome
    }
}

impl RecordableTurn {
    pub(crate) fn user_prompt(prompt: UserPrompt) -> Self {
        Self::InputMessage { content: vec![InputContent::Text { text: prompt.message.to_string() }] }
    }
}

impl TryFrom<RecordableTurn> for provider::Message {
    type Error = ();

    fn try_from(value: RecordableTurn) -> Result<Self, Self::Error> {
        match value {
            RecordableTurn::AssistantMessage { outcome, .. } => {
                match outcome {
                    AssistantTurnOutcome::Completed { response } => Ok(response.into()),
                    _ => Err(()), // TODO: Improve error handling.
                }
            },
            RecordableTurn::InputMessage { .. } => Err(()),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecordableEvent {
    ToolCallRequested {
        tool_call_id: ToolCallId,
        tool_name: String,
        arguments: String
    },
    ToolAuthorization {
        tool_call_id: ToolCallId,
        decision: ToolDecision,
        source: AuthorizationSource,
        reason: Option<String>,
    },
    ToolCallResult(ToolCallResult)
}

impl From<provider::ToolCallRequest> for RecordableEvent {
    fn from(request: provider::ToolCallRequest) -> Self {
        Self::ToolCallRequested {
            tool_call_id: request.id.into(),
            tool_name: request.name,
            arguments: request.arguments,
        }
    }
}

impl From<ToolCallResult> for RecordableEvent {
    fn from(result: ToolCallResult) -> Self {
        Self::ToolCallResult(result)
    }
}

/// Outcome of an assistant turn
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantTurnOutcome {
    /// The assistant turn completed successfully, and the response is available.
    Completed {
        response: AssistantResponse
    },
    /// The assistant turn was interrupted by the user. There may be a partial response
    Interrupted {
        partial_response: Option<AssistantResponse>,
    },
    /// The assistant turn failed midway.
    Failed, // TODO: Take error reason
}

fn uuid_v7_from(dt: DateTime<Utc>) -> Uuid {
    let secs = dt.timestamp() as u64;
    let nanos = dt.timestamp_subsec_nanos();
    Uuid::new_v7(Timestamp::from_unix(NoContext, secs, nanos))
}
