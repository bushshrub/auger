use std::cmp::PartialEq;
use std::collections::HashMap;
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::{NoContext, Timestamp, Uuid};
use crate::{SessionId};
use getset::{CopyGetters, Getters};
use provider::{ToolCallRequest, ToolResult, UserPrompt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    provider: String,
    id: String,
}

impl ModelInfo {
    pub(crate) fn new(provider: String, id: String) -> Self {
        ModelInfo { provider, id }
    }
}

/// A record of an auger session
#[derive(Serialize, Deserialize, Debug, Clone, Getters, CopyGetters)]
pub struct SessionRecord {
    #[getset(get_copy = "pub")]
    session_id: SessionId,
    /// The root "event ID"
    #[getset(get_copy = "pub")]
    root_id: TurnId,
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    turns: HashMap<TurnId, TurnRecord>,
    model_info: ModelInfo,

    previous_turn_id: TurnId
}

impl SessionRecord {
    /// Initialize a new session record. This should be called
    /// at the start of the session.
    pub(crate) fn new(session_id: SessionId, cwd: PathBuf, model_info: ModelInfo) -> Self {
        let created_at = Utc::now();
        let turns = HashMap::new();
        let root_id = TurnId::new(created_at);
        Self {
            session_id,
            root_id,
            created_at,
            cwd,
            turns,
            model_info,
            previous_turn_id: root_id,
        }
    }

    pub fn get_turn(&self, turn_id: &TurnId) -> Option<&TurnRecord> {
        self.turns.get(turn_id)
    }

    pub fn get_turn_mut(&mut self, turn_id: &TurnId) -> Option<&mut TurnRecord> {
        self.turns.get_mut(turn_id)
    }

    pub fn get_previous_turn(&self) -> Option<&TurnRecord> {
        // should only be None if the session JUST started.
        self.get_turn(&self.previous_turn_id)
    }

    pub fn get_previous_turn_mut(&mut self) -> Option<&mut TurnRecord> {
        self.get_turn_mut(&self.previous_turn_id.clone())
    }

    pub fn record_turn(&mut self, turn: RecordableTurn) -> Result<TurnId, ()> {
        let previous_turn = self.turns.get(&self.previous_turn_id);
        match previous_turn {
            Some(prev_turn) => {
                match (&turn, &prev_turn.turn) {
                    (RecordableTurn::InputMessage {..}, RecordableTurn::AssistantMessage {..}) | (RecordableTurn::AssistantMessage {..}, RecordableTurn::InputMessage {..}) => {
                        let tr = TurnRecord::new(turn, self.previous_turn_id);
                        self.previous_turn_id = tr.turn_id;
                        Ok(tr.turn_id)
                    }
                    // TODO: better error information about mismatch.
                    _ => {Err(())}
                }
            },
            None => {
                // This should only happen if the session just started and this is the first turn.
                match &turn {
                    RecordableTurn::InputMessage {..} => {
                        let tr = TurnRecord::new(turn, self.previous_turn_id);
                        self.previous_turn_id = tr.turn_id;
                        Ok(tr.turn_id)
                    },
                    _ => Err(())
                }
            }
        }

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

/// A record of an event that occurred during an auger session.
/// Only events that the harness actually processed will be recorded.
#[derive(Serialize, Deserialize, Debug, Clone, CopyGetters, Getters)]
pub struct EventRecord {
    /// The logical parent of this event.
    parent_id: Option<EventId>,
    /// Timestamp at which this event occurred.
    timestamp: DateTime<Utc>,
    /// Id of this event
    event_id: EventId,
    /// The actual event itself
    event: RecordableEvent,
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

// TODO: should be enum, since only assistant turns can technically have events attached to it.
#[derive(Serialize, Deserialize, Debug, Clone, CopyGetters, Getters)]
pub struct TurnRecord {
    /// The ID of the turn.
    turn_id: TurnId,
    timestamp: DateTime<Utc>,
    /// Parent of the turn
    parent_id: TurnId,

    turn: RecordableTurn,
    /// The events that occurred during the turn.
    events: HashMap<EventId, EventRecord>,
}

impl TurnRecord {
    pub(crate) fn new(turn: RecordableTurn, parent_id: TurnId) -> Self {
        let timestamp = Utc::now();
        let turn_id = TurnId::new(timestamp);

        let assistant_content = match &turn {
            RecordableTurn::InputMessage { content: _ } => Vec::new(),
            RecordableTurn::AssistantMessage { status, content} => {
                content.clone()
            },
        };


        let events = assistant_content
            .into_iter()
            .filter_map(|c| {
            match c {
                AssistantContent::ToolCall { id, name, arguments } => {
                    let event = RecordableEvent::ToolCallRequested {
                        tool_call_id: ToolCallId(id.0.clone()),
                        tool_name: name.clone(),
                        arguments: arguments.clone(),
                    };
                    let record = EventRecord::new(None, timestamp, event);
                    Some(record)
                }
                _ => None
            }
        })
            .map(|record| (record.event_id, record)).collect();


        Self {
            turn_id,
            timestamp,
            parent_id,
            turn,
            events,
        }

    }

    fn record_event(&mut self, event: RecordableEvent, parent_id: Option<EventId>) -> Result<EventId, ()> {
        match &self.turn {
            RecordableTurn::InputMessage { .. } => {
                Err(())
            }
            RecordableTurn::AssistantMessage { status, .. } => {
                match status {
                    AssistantStatus::Completed => {
                        let event_id = EventId::new(self.timestamp);
                        let record = EventRecord::new(parent_id, self.timestamp, event);
                        self.events.insert(record.event_id, record);
                        Ok(event_id)
                    }
                    _ => {
                        Err(())
                    }
                }
            }
        }
    }

    fn get_tool_call_event_id(&self, tool_call_id: &ToolCallId) -> Option<EventId> {
        self.events.iter().find_map(|(event_id, event)| {
            let record_type = &event.event;
            match record_type {
                RecordableEvent::ToolCallRequested { tool_call_id: id, .. } if id == tool_call_id => Some(*event_id),
                _ => None
            }
        })
    }

    pub(crate) fn record_tool_result(&mut self, tool_result: ToolResult) -> Result<EventId, ()> {
        let tool_call_id = ToolCallId(tool_result.tool_call_id().clone());
        match self.get_tool_call_event_id(&tool_call_id) {
            Some(id) => self.record_event(tool_result.into(), Some(id)),
            None => Err(())
        }
    }

    pub(crate) fn record_tool_decision(&mut self, tool_call_id: ToolCallId, decision: bool, source: AuthorizationSource, reason: Option<String>) -> Result<EventId, ()> {
        let tool_call_id = ToolCallId(tool_call_id.0.clone());

        match self.get_tool_call_event_id(&tool_call_id) {
            Some(id) => {
                let event = RecordableEvent::ToolAuthorization {
                    tool_call_id,
                    decision: if decision { ToolDecision::Approved } else { ToolDecision::Denied },
                    source,
                    reason,
                };
                self.record_event(event, Some(id))
            }
            None => Err(())
        }


    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolCallId(String);

// TODO: Should remove and just use ToolCallId type throughout.
impl From<String> for ToolCallId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolData {
    Text {
        text: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolDecision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallStatus {
    /// The tool call executed successfully and returned a result
    Success,
    /// User denied the tool call
    Denied,
    /// The tool was executed, but the tool itself returned a failure.
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthorizationSource {
    User,
    Policy,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
pub enum RecordableTurn {
    /// Input message from the harness. May be due to automatic - returning tool results, or just user sending message.
    InputMessage {
        content: Vec<InputContent>
    },
    /// Result emitted by the clanker.
    AssistantMessage {
        status: AssistantStatus,
        content: Vec<AssistantContent>,
    }
}

impl RecordableTurn {
    pub(crate) fn user_prompt(prompt: UserPrompt) -> Self {
        Self::InputMessage { content: vec![InputContent::Text { text: prompt.message().to_string() }] }
    }

    // TODO: better error type
    pub(crate) fn assistant_message(status: AssistantStatus, msg: provider::Message) -> Result<Self, ()> {
        match msg {
            provider::Message::Assistant { reasoning, content, tool_calls } => {
                let mut assistant_content = Vec::new();
                if let Some(reasoning) = reasoning {
                    assistant_content.push(AssistantContent::Reasoning { text: reasoning });
                }
                assistant_content.push(AssistantContent::Text { text: content });
                for tool_call in tool_calls {
                    assistant_content.push(tool_call.into());
                }
                Ok(Self::AssistantMessage { status, content: assistant_content })
            },
            _ => Err(())
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
    ToolCallResult {
        tool_call_id: ToolCallId,
        // TODO: Should we log the tool status?
        content: Vec<ToolData>
    }
}

impl RecordableEvent {

    pub(crate) fn tool_decision(call_id: String, approved: bool, source: AuthorizationSource, reason: Option<String>) -> Self {
        Self::ToolAuthorization {
            tool_call_id: ToolCallId(call_id),
            decision: if approved { ToolDecision::Approved } else { ToolDecision::Denied },
            source,
            reason,
        }
    }
}

impl From<ToolResult> for RecordableEvent {
    fn from(result: ToolResult) -> Self {
        Self::ToolCallResult {
            tool_call_id: ToolCallId(result.tool_call_id().clone()),
            content: vec![ToolData::Text {text: result.content().clone()}],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantContent {
    Reasoning {
        text: String,
    },
    Text {
        text: String,
    },
    ToolCall {
        id: ToolCallId,
        name: String,
        arguments: String,
    },
}

impl From<ToolCallRequest> for AssistantContent {
    fn from(value: ToolCallRequest) -> Self {
        Self::ToolCall {
            id: ToolCallId(value.id),
            name: value.name,
            arguments: value.arguments,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssistantStatus {
    Completed,
    Interrupted,
    Failed, // TODO: Take error reason
}

fn uuid_v7_from(dt: DateTime<Utc>) -> Uuid {
    let secs = dt.timestamp() as u64;
    let nanos = dt.timestamp_subsec_nanos();
    Uuid::new_v7(Timestamp::from_unix(NoContext, secs, nanos))
}