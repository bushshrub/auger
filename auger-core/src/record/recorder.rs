//! Module for recording session events and turns,
//! and for providing hooks for external observers to be notified of new events
//! and turns.
use crate::harness::tools::tool_execution::ToolCallResult;
use crate::record::SessionRecord;
use crate::record::event::AuthorizationSource;
use crate::record::event::EventId;
use crate::record::event::EventRecord;
use crate::record::event::RecordableEvent;
use crate::record::event::ToolDecision;
use crate::record::turn::AssistantTurnOutcome;
use crate::record::turn::RecordedInput;
use crate::record::turn::TurnId;
use crate::record::turn::TurnRecord;
use auger_driver::Prompt;
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

    /// Record an input against the open input turn, opening one if needed.
    pub fn record_input(&mut self, entry: RecordedInput) -> TurnId {
        let turn_record = self.record.add_input(entry);
        let turn_id = turn_record.data().turn_id();
        if let Some(on_turn) = self.on_turn.0.clone() {
            on_turn(turn_id, &turn_record);
        }
        turn_id
    }

    /// Record a prompt from the user or the harness.
    pub fn record_prompt(&mut self, prompt: Prompt) -> TurnId {
        let entry = match prompt {
            Prompt::User(user) => RecordedInput::User(user),
            Prompt::Harness(message) => RecordedInput::Harness(message),
        };
        self.record_input(entry)
    }

    pub fn record_assistant(&mut self, outcome: AssistantTurnOutcome) -> Result<TurnId, ()> {
        let turn_record = self.record.add_assistant(outcome)?;
        let turn_id = turn_record.data().turn_id();
        if let Some(on_turn) = self.on_turn.0.clone() {
            on_turn(turn_id, &turn_record);
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

    pub(crate) fn record_tool_result(&mut self, tool_result: ToolCallResult) -> TurnId {
        self.record_input(RecordedInput::ToolResult(tool_result))
    }

    pub(crate) fn record_tool_decision(
        &mut self,
        turn_id: TurnId,
        tool_call_id: ToolCallId,
        decision: bool,
        source: AuthorizationSource,
        reason: Option<String>,
    ) -> Result<EventId, ()> {
        let decision = if decision {
            ToolDecision::Approved
        } else {
            ToolDecision::Denied
        };

        let event = RecordableEvent::ToolAuthorization {
            tool_call_id,
            decision,
            source,
            reason,
        };
        self.record_event(turn_id, event, None)
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
