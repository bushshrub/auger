use crate::harness::tools::tool_execution::ToolCallResult;
use crate::record::event::EventId;
use crate::record::event::EventRecord;
use crate::record::event::RecordableEvent;
use chrono::DateTime;
use chrono::Utc;
use derive_more::From;
use getset::CopyGetters;
use getset::Getters;
use provider::AssistantResponse;
use provider::LlmError;
use provider::UserPrompt;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

/// ID of a turn in an auger session. A turn is something like user/assistant
/// etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TurnId(Uuid);

impl TurnId {
    pub(crate) fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Into<Uuid> for TurnId {
    fn into(self) -> Uuid {
        self.0
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

// TODO: should be enum, since only assistant turns can technically have events
// attached to it.
#[derive(Debug, Clone, CopyGetters, Getters)]
pub struct TurnRecord {
    #[getset(get = "pub")]
    data: TurnData,
    /// The events that occurred during the turn.
    #[getset(get = "pub")]
    events: Vec<EventRecord>,
}

impl TurnRecord {
    pub(super) fn new(turn: RecordableTurn, parent_id: Option<TurnId>) -> Self {
        let timestamp = Utc::now();
        let data = TurnData {
            turn_id: TurnId::new(),
            timestamp,
            parent_id,
            turn,
        };
        Self {
            data,
            events: Vec::new(),
        }
    }

    pub(crate) fn from_parts(data: TurnData, events: Vec<EventRecord>) -> Self {
        Self { data, events }
    }

    /// The entries of this turn, if it is an input turn that is still open to
    /// further input.
    pub(super) fn input_entries_mut(&mut self) -> Option<&mut Vec<RecordedInput>> {
        match &mut self.data.turn {
            RecordableTurn::Input { entries } => Some(entries),
            RecordableTurn::Assistant { .. } => None,
        }
    }

    pub(crate) fn restore_event(&mut self, event: EventRecord) {
        self.events.push(event);
    }

    pub(crate) fn add_event(
        &mut self,
        event: RecordableEvent,
        parent_id: Option<EventId>,
    ) -> Result<EventRecord, ()> {
        match &self.data.turn {
            RecordableTurn::Input { .. } => Err(()),
            RecordableTurn::Assistant { outcome } => match outcome {
                AssistantTurnOutcome::Completed { .. } => {
                    let ts = Utc::now();
                    let record = EventRecord::new(parent_id, ts, event);
                    self.events.push(record.clone());
                    Ok(record)
                }
                _ => Err(()),
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecordableTurn {
    /// Input message from the harness. May be due to automatic - returning tool
    /// results, or just user sending message.
    Input { entries: Vec<RecordedInput> },
    /// Result emitted by the clanker.
    Assistant { outcome: AssistantTurnOutcome },
}

/// An input recorded against a turn. Mirrors the driver's `InputEntry`, but
/// keeps the whole tool call result rather than the flattened form the model
/// gets.
#[derive(Serialize, Deserialize, Debug, Clone, From)]
#[serde(rename_all = "snake_case")]
pub enum RecordedInput {
    User(UserPrompt),
    Harness(String),
    ToolResult(ToolCallResult),
}

/// Outcome of an assistant turn
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantTurnOutcome {
    /// The assistant turn completed successfully, and the response is
    /// available.
    Completed { response: AssistantResponse },
    /// The assistant turn never settled. There may be a partial response.
    Incomplete {
        partial_response: Option<AssistantResponse>,
        reason: StopReason,
    },
}

/// Why an assistant turn did not settle.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The user interrupted it.
    Interrupted,
    /// The stream failed midway, or never started.
    Failed(LlmError),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Externally tagged: serde cannot internally tag a newtype variant that
    /// wraps a string, so `Harness` would fail to serialize at runtime.
    #[test]
    fn recorded_input_is_externally_tagged() {
        let user = RecordedInput::User(UserPrompt::new("hi".to_string()));
        assert_eq!(
            serde_json::to_string(&user).unwrap(),
            r#"{"user":{"message":"hi"}}"#
        );
        let harness = RecordedInput::Harness("steer".to_string());
        assert_eq!(
            serde_json::to_string(&harness).unwrap(),
            r#"{"harness":"steer"}"#
        );
    }
}
