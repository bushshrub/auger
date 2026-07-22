use std::cmp::PartialEq;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
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
    #[getset(get_copy = "pub")]
    created_at: DateTime<Utc>,
    cwd: PathBuf,
    turns: HashMap<TurnId, TurnRecord>,
    model_info: ModelInfo,
    previous_turn_id: TurnId,
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
            previous_turn_id: root_id
        }
    }

    pub(super) fn from_trace_parts(
        session_id: SessionId,
        root_id: TurnId,
        created_at: DateTime<Utc>,
        cwd: PathBuf,
        model_info: ModelInfo,
        turns: HashMap<TurnId, TurnRecord>,
        previous_turn_id: TurnId,
    ) -> Self {
        Self { session_id, root_id, created_at, cwd, turns, model_info, previous_turn_id }
    }

    pub fn get_turn(&self, turn_id: &TurnId) -> Option<&TurnRecord> {
        self.turns.get(turn_id)
    }

    pub fn get_turn_mut(&mut self, turn_id: &TurnId) -> Option<&mut TurnRecord> {
        self.turns.get_mut(turn_id)
    }

    pub fn turns(&self) -> impl Iterator<Item = &TurnRecord> {
        let mut turns = Vec::with_capacity(self.turns.len());
        let mut turn_id = self.previous_turn_id;

        while turn_id != self.root_id {
            let Some(turn) = self.turns.get(&turn_id) else {
                break;
            };
            turn_id = turn.parent_id();
            turns.push(turn);
        }
        turns.reverse();
        turns.into_iter()
    }

    pub fn get_previous_turn(&self) -> Option<&TurnRecord> {
        // should only be None if the session JUST started.
        self.get_turn(&self.previous_turn_id)
    }

    pub fn get_previous_turn_mut(&mut self) -> Option<&mut TurnRecord> {
        self.get_turn_mut(&self.previous_turn_id.clone())
    }

    pub(crate) fn add_turn(&mut self, turn: RecordableTurn) -> Result<TurnRecord, ()> {
        let previous_turn = self.turns.get(&self.previous_turn_id);
        match previous_turn {
            Some(prev_turn) => {
                match (&turn, &prev_turn.turn) {
                    (RecordableTurn::InputMessage {..}, RecordableTurn::AssistantMessage {..}) | (RecordableTurn::AssistantMessage {..}, RecordableTurn::InputMessage {..}) => {
                        let tr = TurnRecord::new(turn, self.previous_turn_id);
                        self.previous_turn_id = tr.turn_id;
                        self.turns.insert(tr.turn_id, tr.clone());
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
                        let tr = TurnRecord::new(turn, self.previous_turn_id);
                        self.previous_turn_id = tr.turn_id;
                        self.turns.insert(tr.turn_id, tr.clone());
                        Ok(tr)
                    },
                    _ => Err(())
                }
            }
        }
    }

    /// The full trace-record stream for this session in JSONL order: the
    /// session header, then each turn followed by the events recorded during
    /// it. This is the exact shape persisted to `trace.jsonl`, so the snapshot
    /// API and the on-disk trace share one representation.
    pub fn trace_records(&self) -> Vec<auger_traces::schema::TraceRecord> {
        use auger_traces::schema as trace;
        let mut records = Vec::new();
        let root_turn_id: Uuid = self.root_id.into();
        let header = trace::SessionHeader::new(
            1,
            self.session_id.as_uuid(),
            trace::TurnId::from(root_turn_id),
            self.created_at,
            self.cwd.clone(),
            trace::ModelInfo::new(self.model_info.provider.clone(), self.model_info.id.clone()),
        );
        records.push(trace::TraceRecord::Session(header));
        for turn in self.turns() {
            records.push(trace::TraceRecord::Turn(turn.clone().into()));
            let events: Vec<trace::EventRecord> = turn.into();
            records.extend(events.into_iter().map(trace::TraceRecord::Event));
        }
        records
    }

    pub fn as_messages(&self) -> Vec<provider::Message> {
        let mut messages = Vec::new();
        let mut curr_turn_id = self.previous_turn_id;
        loop {
            let turn = self.get_turn(&curr_turn_id);
            if let Some(turn) = turn {
                match turn.turn() {
                    RecordableTurn::InputMessage { content } => {
                        let mut msg = String::new();
                        let tool_calls = content.iter().filter_map(|c| {
                            match c {
                                InputContent::Text { text } => {
                                    msg.push_str(text.as_str());
                                    None
                                }
                                InputContent::ToolResult { tool_call_id, content } => {
                                    let tool_result_content = content.iter().fold(String::new(), |mut acc, c| {
                                        match c {
                                            ToolData::Text { text } => {
                                                acc.push_str(text.as_str());
                                            }
                                        }
                                        acc
                                    });
                                    Some(ToolResult::new(tool_call_id.clone().0, tool_result_content))
                                }
                            }
                        }).collect();
                        messages.push(provider::Message::User {
                            message: UserPrompt::new(msg),
                            tool_call_results: tool_calls
                        })
                    }
                    RecordableTurn::AssistantMessage { .. } => {
                        messages.push(turn.turn().clone().try_into().expect("Failed to convert turn to message"));
                    }
                }
                curr_turn_id = turn.parent_id();
            } else {
                break;
            }
        }
        messages.reverse();
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
    pub(crate) fn new(parent_id: Option<EventId>, timestamp: DateTime<Utc>, event: RecordableEvent) -> Self {
        let event_id = EventId::new(timestamp);
        Self {
            parent_id,
            timestamp,
            event_id,
            event,
        }
    }


    pub(super) fn from_trace_parts(event_id: EventId, parent_id: Option<EventId>, timestamp: DateTime<Utc>, event: RecordableEvent) -> Self {
        Self { parent_id, timestamp, event_id, event }
    }

}

// TODO: should be enum, since only assistant turns can technically have events attached to it.
#[derive(Serialize, Deserialize, Debug, Clone, CopyGetters, Getters)]
pub struct TurnRecord {
    /// The ID of the turn.
    #[getset(get_copy = "pub")]
    turn_id: TurnId,
    #[getset(get = "pub")]
    timestamp: DateTime<Utc>,
    /// Parent of the turn
    #[getset(get_copy = "pub")]
    parent_id: TurnId,
    #[getset(get = "pub")]
    turn: RecordableTurn,
    /// The events that occurred during the turn.
    #[getset(get = "pub")]
    events: HashMap<EventId, EventRecord>,
}

impl TurnRecord {
    fn new(turn: RecordableTurn, parent_id: TurnId) -> Self {
        let timestamp = Utc::now();
        let turn_id = TurnId::new(timestamp);


        Self {
            turn_id,
            timestamp,
            parent_id,
            turn,
            events: HashMap::new(),
        }

    }

    pub(super) fn from_trace_parts(
        turn_id: TurnId,
        timestamp: DateTime<Utc>,
        parent_id: TurnId,
        turn: RecordableTurn,
        events: HashMap<EventId, EventRecord>,
    ) -> Self {
        Self { turn_id, timestamp, parent_id, turn, events }
    }

    pub(crate) fn add_event(&mut self, event: RecordableEvent, parent_id: Option<EventId>) -> Result<EventRecord, ()> {
        match &self.turn {
            RecordableTurn::InputMessage { .. } => {
                Err(())
            }
            RecordableTurn::AssistantMessage { status, .. } => {
                match status {
                    AssistantStatus::Completed => {
                        let ts = Utc::now();
                        let record = EventRecord::new(parent_id, ts, event);
                        self.events.insert(record.event_id, record.clone());
                        Ok(record)
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

    pub(crate) fn record_tool_result(&mut self, tool_result: ToolResult) -> Result<EventRecord, ()> {
        let tool_call_id = ToolCallId(tool_result.tool_call_id.clone());
        match self.get_tool_call_event_id(&tool_call_id) {
            Some(id) => self.add_event(tool_result.into(), Some(id)),
            None => Err(())
        }
    }

    pub(crate) fn record_tool_decision(&mut self, tool_call_id: ToolCallId, decision: ToolDecision, source: AuthorizationSource, reason: Option<String>) -> Result<EventRecord, ()> {
        let tool_call_id = ToolCallId(tool_call_id.0.clone());
        match self.get_tool_call_event_id(&tool_call_id) {
            Some(id) => {
                let event = RecordableEvent::ToolAuthorization {
                    tool_call_id,
                    decision,
                    source,
                    reason,
                };
                self.add_event(event, Some(id))
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

impl From<ToolCallId> for String {
    fn from(id: ToolCallId) -> Self {
        id.0
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
        Self::InputMessage { content: vec![InputContent::Text { text: prompt.message.to_string() }] }
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

impl TryFrom<RecordableTurn> for provider::Message {
    type Error = ();

    fn try_from(value: RecordableTurn) -> Result<Self, Self::Error> {
        match value {
            RecordableTurn::AssistantMessage { content, .. } => {
                let mut reasoning = None;
                let mut text = String::new();
                let mut tool_calls = Vec::new();
                for item in content {
                    match item {
                        AssistantContent::Reasoning { text: r } => reasoning = Some(r),
                        AssistantContent::Text { text: t } => text = t,
                        AssistantContent::ToolCall { id, name, arguments } => {
                            tool_calls.push(ToolCallRequest { id: id.0, name, arguments });
                        }
                    }
                }
                Ok(provider::Message::Assistant { reasoning, content: text, tool_calls })
            }
            _ => Err(())
        }
    }
}

impl TryFrom<provider::Message> for RecordableTurn {
    type Error = ();

    fn try_from(value: provider::Message) -> Result<Self, Self::Error> {
        match value {
            provider::Message::Assistant { reasoning, content, tool_calls } => {
                let mut assistant_content = Vec::new();
                if let Some(reasoning) = reasoning {
                    assistant_content.push(AssistantContent::Reasoning { text: reasoning });
                }
                assistant_content.push(AssistantContent::Text { text: content });
                for tool_call in tool_calls {
                    assistant_content.push(tool_call.into());
                }
                Ok(Self::AssistantMessage { status: AssistantStatus::Completed, content: assistant_content })
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

impl From<ToolResult> for RecordableEvent {
    fn from(result: ToolResult) -> Self {
        Self::ToolCallResult {
            tool_call_id: ToolCallId(result.tool_call_id.clone()),
            content: vec![ToolData::Text {text: result.content.clone()}],
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
