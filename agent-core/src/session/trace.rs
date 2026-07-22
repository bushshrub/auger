use super::history::{
    AssistantContent, AssistantStatus, AuthorizationSource, EventId, EventRecord, InputContent,
    ModelInfo, RecordableEvent, RecordableTurn, SessionRecord, ToolCallId, ToolData, ToolDecision,
    TurnEvent, TurnId, TurnRecord,
};
use super::SessionId;
use auger_traces::schema as trace;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TraceRestoreError {
    #[error("trace is empty")]
    Empty,
    #[error("first record must be a session header")]
    MissingSessionHeader,
    #[error("unsupported trace version {0}")]
    UnsupportedVersion(u32),
    #[error("trace contains more than one session header")]
    DuplicateSessionHeader,
    #[error("duplicate turn {0}")]
    DuplicateTurn(Uuid),
    #[error("turn {turn_id} does not follow parent {expected_parent}")]
    InvalidTurnParent { turn_id: Uuid, expected_parent: Uuid },
    #[error("duplicate event {0}")]
    DuplicateEvent(Uuid),
    #[error("event {event_id} refers to unknown turn {turn_id}")]
    UnknownEventTurn { event_id: Uuid, turn_id: Uuid },
    #[error("event {event_id} refers to unknown parent event {parent_event_id}")]
    UnknownParentEvent { event_id: Uuid, parent_event_id: Uuid },
}

struct RestoredTurn {
    timestamp: chrono::DateTime<chrono::Utc>,
    parent_id: TurnId,
    turn: RecordableTurn,
    events: HashMap<EventId, EventRecord>,
}

impl SessionRecord {
    pub fn restore(items: Vec<trace::TraceRecord>) -> Result<Self, TraceRestoreError> {
        let mut items = items.into_iter();
        let header = match items.next() {
            Some(trace::TraceRecord::Session(header)) => header,
            Some(_) => return Err(TraceRestoreError::MissingSessionHeader),
            None => return Err(TraceRestoreError::Empty),
        };
        if *header.version() != 1 {
            return Err(TraceRestoreError::UnsupportedVersion(*header.version()));
        }

        let root_id: TurnId = Uuid::from(*header.root_turn_id()).into();
        let mut previous_turn_id = root_id;
        let mut is_first_turn = true;
        let mut turns = HashMap::new();
        let mut event_ids = HashSet::new();

        for item in items {
            match item {
                trace::TraceRecord::Session(_) => return Err(TraceRestoreError::DuplicateSessionHeader),
                trace::TraceRecord::Turn(turn) => {
                    let turn_id: TurnId = Uuid::from(*turn.id()).into();
                    let turn_uuid: Uuid = turn_id.into();
                    if turns.contains_key(&turn_id) {
                        return Err(TraceRestoreError::DuplicateTurn(turn_uuid));
                    }
                    let parent_id = turn.parent_turn_id().map(|id| TurnId::from(Uuid::from(id)));
                    let valid_parent = if is_first_turn {
                        parent_id.is_none() || parent_id == Some(root_id)
                    } else {
                        parent_id == Some(previous_turn_id)
                    };
                    if !valid_parent {
                        return Err(TraceRestoreError::InvalidTurnParent {
                            turn_id: turn_uuid,
                            expected_parent: previous_turn_id.into(),
                        });
                    }
                    turns.insert(turn_id, RestoredTurn {
                        timestamp: *turn.timestamp(),
                        parent_id: previous_turn_id,
                        turn: restore_turn(turn.turn().clone()),
                        events: HashMap::new(),
                    });
                    previous_turn_id = turn_id;
                    is_first_turn = false;
                }
                trace::TraceRecord::Event(event) => {
                    let event_id: EventId = Uuid::from(*event.id()).into();
                    let turn_id: TurnId = Uuid::from(*event.turn_id()).into();
                    let event_uuid: Uuid = event_id.into();
                    if !event_ids.insert(event_id) {
                        return Err(TraceRestoreError::DuplicateEvent(event_uuid));
                    }
                    let Some(turn) = turns.get_mut(&turn_id) else {
                        return Err(TraceRestoreError::UnknownEventTurn {
                            event_id: event_uuid,
                            turn_id: turn_id.into(),
                        });
                    };
                    let parent_id = event.parent_event_id().map(|id| EventId::from(Uuid::from(id)));
                    if let Some(parent_id) = parent_id {
                        if !turn.events.contains_key(&parent_id) {
                            return Err(TraceRestoreError::UnknownParentEvent {
                                event_id: event_uuid,
                                parent_event_id: parent_id.into(),
                            });
                        }
                    }
                    turn.events.insert(event_id, EventRecord::from_trace_parts(
                        event_id,
                        parent_id,
                        *event.timestamp(),
                        restore_event(event.event().clone()),
                    ));
                }
            }
        }

        let turns = turns.into_iter().map(|(turn_id, turn)| {
            (turn_id, TurnRecord::from_trace_parts(
                turn_id,
                turn.timestamp,
                turn.parent_id,
                turn.turn,
                turn.events,
            ))
        }).collect();
        Ok(SessionRecord::from_trace_parts(
            SessionId::from(*header.session_id()),
            root_id,
            *header.created_at(),
            header.cwd().clone(),
            ModelInfo::new(header.model().provider().clone(), header.model().id().clone()),
            turns,
            previous_turn_id,
        ))
    }
}

fn restore_turn(turn: trace::Turn) -> RecordableTurn {
    match turn {
        trace::Turn::InputMessage(message) => RecordableTurn::InputMessage {
            content: message.content().iter().cloned().map(restore_input_content).collect(),
        },
        trace::Turn::AssistantMessage(message) => RecordableTurn::AssistantMessage {
            status: restore_status(message.status()),
            content: message.content().iter().cloned().map(restore_assistant_content).collect(),
        },
    }
}

fn restore_input_content(content: trace::InputContent) -> InputContent {
    match content {
        trace::InputContent::Text(text) => InputContent::Text { text: text.text().clone() },
        trace::InputContent::ToolResult(result) => InputContent::ToolResult {
            tool_call_id: String::from(result.tool_call_id().clone()).into(),
            content: result.content().iter().cloned().map(restore_tool_data).collect(),
        },
    }
}

fn restore_assistant_content(content: trace::AssistantContent) -> AssistantContent {
    match content {
        trace::AssistantContent::Reasoning(text) => AssistantContent::Reasoning { text: text.text().clone() },
        trace::AssistantContent::Text(text) => AssistantContent::Text { text: text.text().clone() },
        trace::AssistantContent::ToolCall(call) => AssistantContent::ToolCall {
            id: String::from(call.id().clone()).into(),
            name: call.name().clone(),
            arguments: call.arguments().clone(),
        },
    }
}

fn restore_status(status: &trace::AssistantStatus) -> AssistantStatus {
    match status {
        trace::AssistantStatus::Completed => AssistantStatus::Completed,
        trace::AssistantStatus::Interrupted => AssistantStatus::Interrupted,
        trace::AssistantStatus::Failed => AssistantStatus::Failed,
    }
}

fn restore_tool_data(data: trace::ToolData) -> ToolData {
    match data {
        trace::ToolData::Text(text) => ToolData::Text { text: text.text().clone() },
    }
}

fn restore_event(event: trace::Event) -> RecordableEvent {
    match event {
        trace::Event::ToolCallRequested(event) => RecordableEvent::ToolCallRequested {
            tool_call_id: String::from(event.tool_call_id().clone()).into(),
            tool_name: event.tool_name().clone(),
            arguments: event.arguments().clone(),
        },
        trace::Event::ToolAuthorization(event) => RecordableEvent::ToolAuthorization {
            tool_call_id: String::from(event.tool_call_id().clone()).into(),
            decision: match event.decision() {
                trace::ToolDecision::Approved => ToolDecision::Approved,
                trace::ToolDecision::Denied => ToolDecision::Denied,
            },
            source: match event.source() {
                trace::AuthorizationSource::User => AuthorizationSource::User,
                trace::AuthorizationSource::Policy => AuthorizationSource::Policy,
            },
            reason: event.reason().clone(),
        },
        trace::Event::ToolCallResult(event) => RecordableEvent::ToolCallResult {
            tool_call_id: String::from(event.tool_call_id().clone()).into(),
            content: event.content().iter().cloned().map(restore_tool_data).collect(),
        },
    }
}

impl From<TurnRecord> for trace::TurnRecord {
    fn from(record: TurnRecord) -> Self {
        let turn_id: Uuid = record.turn_id().into();
        let parent_id: Uuid = record.parent_id().into();
        trace::TurnRecord::new(
            trace::TurnId::from(turn_id),
            Some(trace::TurnId::from(parent_id)),
            record.timestamp().clone(),
            record.turn().clone().into(),
        )
    }
}

impl From<&TurnRecord> for Vec<trace::EventRecord> {
    fn from(record: &TurnRecord) -> Self {
        let mut events: Vec<_> = record.events().values().cloned().collect();
        // Events are stored in a map; replay them in the order they occurred so
        // the snapshot matches the on-disk trace (event ids are time-ordered).
        events.sort_by_key(|event| event.event_id());
        events
            .into_iter()
            .map(|event| TurnEvent::new(record.turn_id(), event).into())
            .collect()
    }
}

impl From<TurnEvent> for trace::EventRecord {
    fn from(event: TurnEvent) -> Self {
        let event_id: Uuid = event.record().event_id().into();
        let turn_id: Uuid = event.turn_id().into();
        let parent_id = event.record().parent_id().map(|id| {
            let id: Uuid = id.into();
            trace::EventId::from(id)
        });
        trace::EventRecord::new(
            trace::EventId::from(event_id),
            trace::TurnId::from(turn_id),
            parent_id,
            event.record().timestamp().clone(),
            event.record().event().clone().into(),
        )
    }
}

impl From<RecordableTurn> for trace::Turn {
    fn from(turn: RecordableTurn) -> Self {
        match turn {
            RecordableTurn::InputMessage { content } => {
                Self::InputMessage(trace::InputMessage::new(content.into_iter().map(Into::into).collect()))
            }
            RecordableTurn::AssistantMessage { status, content } => {
                Self::AssistantMessage(trace::AssistantMessage::new(
                    status.into(),
                    content.into_iter().map(Into::into).collect(),
                ))
            }
        }
    }
}

impl From<InputContent> for trace::InputContent {
    fn from(content: InputContent) -> Self {
        match content {
            InputContent::Text { text } => Self::Text(trace::TextData::new(text)),
            InputContent::ToolResult { tool_call_id, content } => {
                Self::ToolResult(trace::InputToolResult::new(
                    tool_call_id.into(),
                    content.into_iter().map(Into::into).collect(),
                ))
            }
        }
    }
}

impl From<AssistantContent> for trace::AssistantContent {
    fn from(content: AssistantContent) -> Self {
        match content {
            AssistantContent::Reasoning { text } => Self::Reasoning(trace::TextData::new(text)),
            AssistantContent::Text { text } => Self::Text(trace::TextData::new(text)),
            AssistantContent::ToolCall { id, name, arguments } => {
                Self::ToolCall(trace::AssistantToolCall::new(id.into(), name, arguments))
            }
        }
    }
}

impl From<ToolData> for trace::ToolData {
    fn from(data: ToolData) -> Self {
        match data {
            ToolData::Text { text } => Self::Text(trace::TextData::new(text)),
        }
    }
}

impl From<ToolCallId> for trace::ToolCallId {
    fn from(id: ToolCallId) -> Self {
        String::from(id).into()
    }
}

impl From<ToolDecision> for trace::ToolDecision {
    fn from(decision: ToolDecision) -> Self {
        match decision {
            ToolDecision::Approved => Self::Approved,
            ToolDecision::Denied => Self::Denied,
        }
    }
}

impl From<AuthorizationSource> for trace::AuthorizationSource {
    fn from(source: AuthorizationSource) -> Self {
        match source {
            AuthorizationSource::User => Self::User,
            AuthorizationSource::Policy => Self::Policy,
        }
    }
}

impl From<RecordableEvent> for trace::Event {
    fn from(event: RecordableEvent) -> Self {
        match event {
            RecordableEvent::ToolCallRequested {
                tool_call_id,
                tool_name,
                arguments,
            } => Self::ToolCallRequested(trace::ToolCallRequested::new(
                tool_call_id.into(),
                tool_name,
                arguments,
            )),
            RecordableEvent::ToolAuthorization {
                tool_call_id,
                decision,
                source,
                reason,
            } => Self::ToolAuthorization(trace::ToolAuthorization::new(
                tool_call_id.into(),
                decision.into(),
                source.into(),
                reason,
            )),
            RecordableEvent::ToolCallResult {
                tool_call_id,
                content,
            } => Self::ToolCallResult(trace::ToolCallResult::new(
                tool_call_id.into(),
                content.into_iter().map(Into::into).collect(),
            )),
        }
    }
}

impl From<AssistantStatus> for trace::AssistantStatus {
    fn from(status: AssistantStatus) -> Self {
        match status {
            AssistantStatus::Completed => Self::Completed,
            AssistantStatus::Interrupted => Self::Interrupted,
            AssistantStatus::Failed => Self::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_session_record_from_trace_records() {
        let input = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../auger-traces/trace_format.jsonl"));
        let records: Vec<trace::TraceRecord> = input
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid trace record"))
            .collect();
        let expected_len = records.len();

        let restored = SessionRecord::restore(records).expect("restorable trace");
        assert_eq!(restored.trace_records().len(), expected_len);
        assert!(matches!(restored.as_messages().first(), Some(provider::Message::User { .. })));
        assert!(matches!(restored.as_messages().last(), Some(provider::Message::Assistant { .. })));
    }
}
