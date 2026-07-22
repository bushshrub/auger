//! Module for recording session events and turns,
//! and for providing hooks for external observers to be notified of new events and turns.
use crate::session::history::{AssistantContent, AuthorizationSource, EventId, EventRecord, RecordableEvent, RecordableTurn, ToolCallId, ToolDecision, TurnId, TurnRecord};
use crate::session::SessionRecord;
use getset::{CloneGetters, Getters};
use provider::ToolResult;
use std::sync::Arc;


type TurnHook = Hook<dyn Fn(TurnId, &TurnRecord)  + Send + Sync>;
type EventHook = Hook<dyn Fn(TurnId, &EventRecord) + Send + Sync>;

pub type TurnCallback = Arc<dyn Fn(TurnId, &TurnRecord)  + Send + Sync>;
pub type EventCallback = Arc<dyn Fn(TurnId, &EventRecord) + Send + Sync>;

struct Hook<T: ?Sized>(Option<Arc<T>>);

#[derive(CloneGetters)]
pub struct SessionRecorder {
    #[getset(get_clone = "pub")]
    record: SessionRecord,

    on_turn:  TurnHook,
    on_event: EventHook,
}

impl SessionRecorder {
    pub(crate) fn new(record: SessionRecord, on_turn: TurnCallback, on_event: EventCallback) -> Self {
        Self {
            record,
            on_turn: Hook(Some(on_turn)),
            on_event: Hook(Some(on_event)),
        }
    }

    pub fn previous_turn_id(&self) -> Option<TurnId> {
        self.record.get_previous_turn().map(|tr| tr.turn_id())
    }

    pub fn record_turn(&mut self, turn: RecordableTurn) -> Result<TurnId, ()> {
        let assistant_content = match &turn {
            RecordableTurn::InputMessage { content: _ } => Vec::new(),
            RecordableTurn::AssistantMessage { status, content} => {
                content.clone()
            },
        };

        let turn_record = self.record.add_turn(turn)?;
        let turn_id = turn_record.turn_id();
        if let Some(on_turn) = self.on_turn.0.clone() {
            on_turn(turn_id, &turn_record);
        }
        let events = assistant_content
            .into_iter()
            .filter_map(|c| {
                match c {
                    AssistantContent::ToolCall { id, name, arguments } => {
                        Some(RecordableEvent::ToolCallRequested {
                            tool_call_id: id,
                            tool_name: name.clone(),
                            arguments: arguments.clone(),
                        })
                    }
                    _ => None
                }
            });
        for event in events {
            let record = self.record.get_turn_mut(&turn_id)
                .expect("turn to have been added")
                .add_event(event, None).expect("event to record event");

            if let Some(on_event) = self.on_event.0.clone() {
                on_event(turn_id, &record);
            }
        }

        Ok(turn_id)
    }

    pub fn record_event(&mut self, turn_id: TurnId, event: RecordableEvent, parent_id: Option<EventId>) -> Result<EventId, ()> {
        let tr = self.record.get_turn_mut(&turn_id).ok_or_else(|| ())?;
        let er =  tr.add_event(event, parent_id)?;
        Ok(er.event_id())
    }

    pub(crate) fn record_tool_result(&mut self, turn_id: TurnId, tool_result: ToolResult) -> Result<EventId, ()> {
        let tr = self.record.get_turn_mut(&turn_id).ok_or_else(|| ())?;
        let er = tr.record_tool_result(tool_result)?;
        if let Some(on_event) = self.on_event.0.clone() {
            on_event(turn_id, &er);
        }
        Ok(er.event_id())
    }

    pub(crate) fn record_tool_decision(&mut self, turn_id: TurnId, tool_call_id: ToolCallId, decision: bool, source: AuthorizationSource, reason: Option<String>) -> Result<EventId, ()> {
        let tr = self.record.get_turn_mut(&turn_id).ok_or_else(|| ())?;
        let decision = if decision { ToolDecision::Approved } else { ToolDecision::Denied };
        let er = tr.record_tool_decision(tool_call_id, decision, source, reason)?;
        if let Some(on_event) = self.on_event.0.clone() {
            on_event(turn_id, &er);
        }
        Ok(er.event_id())
    }


}

impl<T: ?Sized> std::fmt::Debug for Hook<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Hook(set)" } else { "Hook(unset)" })
    }
}
impl<T: ?Sized> Clone for Hook<T> {
    fn clone(&self) -> Self { Hook(self.0.clone()) }  // Arc clone, no T: Clone needed
}

impl<T: ?Sized> Default for Hook<T> {
    fn default() -> Self { Hook(None) }
}