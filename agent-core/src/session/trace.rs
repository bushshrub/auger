use super::history::{
    AssistantContent, AssistantStatus, AuthorizationSource, InputContent,
    RecordableEvent, RecordableTurn, ToolCallId, ToolData, ToolDecision, TurnEvent, TurnRecord,
};
use auger_traces::schema as trace;
use uuid::Uuid;

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
        record
            .events()
            .values()
            .cloned()
            .map(|event| TurnEvent {
                turn_id: record.turn_id(),
                record: event,
            }.into())
            .collect()
    }
}

impl From<TurnEvent> for trace::EventRecord {
    fn from(event: TurnEvent) -> Self {
        let event_id: Uuid = event.record.event_id().into();
        let turn_id: Uuid = event.turn_id.into();
        let parent_id = event.record.parent_id().map(|id| {
            let id: Uuid = id.into();
            trace::EventId::from(id)
        });
        trace::EventRecord::new(
            trace::EventId::from(event_id),
            trace::TurnId::from(turn_id),
            parent_id,
            event.record.timestamp().clone(),
            event.record.event().clone().into(),
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
