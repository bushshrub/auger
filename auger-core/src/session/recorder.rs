//! Module for recording session events and turns,
//! and for providing hooks for external observers to be notified of new events
//! and turns.
use crate::session::SessionRecord;
use crate::session::history::AssistantTurnOutcome;
use crate::session::history::AuthorizationSource;
use crate::session::history::EventId;
use crate::session::history::EventRecord;
use crate::session::history::RecordableEvent;
use crate::session::history::RecordableTurn;
use crate::session::history::ToolDecision;
use crate::session::history::TurnId;
use crate::session::history::TurnRecord;
use crate::tools::tool_execution::ToolCallResult;
use auger_driver::ToolCallId;
use getset::Getters;
use std::sync::Arc;

type TurnHook = Hook<dyn Fn(TurnId, &TurnRecord) + Send + Sync>;
type EventHook = Hook<dyn Fn(TurnId, &EventRecord) + Send + Sync>;

pub type TurnCallback = Arc<dyn Fn(TurnId, &TurnRecord) + Send + Sync>;
pub type EventCallback = Arc<dyn Fn(TurnId, &EventRecord) + Send + Sync>;

struct Hook<T: ?Sized>(Option<Arc<T>>);

#[derive(Getters)]
pub struct SessionRecorder {
    #[getset(get = "pub")]
    record: SessionRecord,

    on_turn: TurnHook,
    on_event: EventHook,
}

impl SessionRecorder {
    pub(crate) fn new(
        record: SessionRecord,
        on_turn: TurnCallback,
        on_event: EventCallback,
    ) -> Self {
        Self {
            record,
            on_turn: Hook(Some(on_turn)),
            on_event: Hook(Some(on_event)),
        }
    }

    pub fn previous_turn_id(&self) -> Option<TurnId> {
        self.record
            .get_previous_turn()
            .map(|tr| tr.data().turn_id())
    }

    pub fn record_turn(&mut self, turn: RecordableTurn) -> Result<TurnId, ()> {
        let tool_calls = match &turn {
            RecordableTurn::InputMessage { content: _ } => Vec::new(),
            RecordableTurn::AssistantMessage { outcome } => {
                match outcome {
                    AssistantTurnOutcome::Completed { response } => response.tool_calls.clone(),
                    // do not record interrupted or failed assistant messages' tool calls.
                    _ => Vec::new(),
                }
            }
        };

        let turn_record = self.record.add_turn(turn)?;
        let turn_id = turn_record.data().turn_id();
        if let Some(on_turn) = self.on_turn.0.clone() {
            on_turn(turn_id, &turn_record);
        }
        let events = tool_calls
            .into_iter()
            .map(RecordableEvent::from)
            .collect::<Vec<_>>();
        for event in events {
            let record = self
                .record
                .get_turn_mut(&turn_id)
                .expect("turn to have been added")
                .add_event(event, None)
                .expect("event to record event");

            if let Some(on_event) = self.on_event.0.clone() {
                on_event(turn_id, &record);
            }
        }

        Ok(turn_id)
    }

    pub fn record_event(
        &mut self,
        turn_id: TurnId,
        event: RecordableEvent,
        parent_id: Option<EventId>,
    ) -> Result<EventId, ()> {
        let tr = self.record.get_turn_mut(&turn_id).ok_or_else(|| ())?;
        let er = tr.add_event(event, parent_id)?;
        if let Some(on_event) = self.on_event.0.clone() {
            on_event(turn_id, &er);
        }
        Ok(er.event_id())
    }

    pub(crate) fn record_tool_result(
        &mut self,
        turn_id: TurnId,
        tool_result: ToolCallResult,
    ) -> Result<EventId, ()> {
        let tr = self.record.get_turn_mut(&turn_id).ok_or(())?;
        let tool_call_id: ToolCallId = tool_result.tool_call_id();
        let tool_decision_event_id = tr.get_tool_decision_event_id(&tool_call_id).ok_or(())?;
        self.record_event(turn_id, tool_result.into(), Some(tool_decision_event_id))
    }

    pub(crate) fn record_tool_decision(
        &mut self,
        turn_id: TurnId,
        tool_call_id: ToolCallId,
        decision: bool,
        source: AuthorizationSource,
        reason: Option<String>,
    ) -> Result<EventId, ()> {
        let tr = self.record.get_turn_mut(&turn_id).ok_or(())?;
        let decision = if decision {
            ToolDecision::Approved
        } else {
            ToolDecision::Denied
        };

        let tool_call_request_event_id = tr.get_tool_call_event_id(&tool_call_id).ok_or(())?;
        let event = RecordableEvent::ToolAuthorization {
            tool_call_id,
            decision,
            source,
            reason,
        };
        self.record_event(turn_id, event, Some(tool_call_request_event_id))
    }
}

impl<T: ?Sized> std::fmt::Debug for Hook<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Hook(set)"
        } else {
            "Hook(unset)"
        })
    }
}
impl<T: ?Sized> Clone for Hook<T> {
    fn clone(&self) -> Self {
        Hook(self.0.clone())
    } // Arc clone, no T: Clone needed
}

impl<T: ?Sized> Default for Hook<T> {
    fn default() -> Self {
        Hook(None)
    }
}
