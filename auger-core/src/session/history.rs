use crate::ids::EventId;
use crate::ids::SessionId;
use crate::ids::TurnId;
use crate::tools::tool_execution::ToolCallResult;
use auger_driver::{HarnessEntry, InputEntry, RestoreState, ToolCallId, Turn};
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
use std::path::PathBuf;

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
    pub fn new(
        session_id: SessionId,
        created_at: DateTime<Utc>,
        cwd: PathBuf,
        model_info: ModelInfo,
    ) -> Self {
        Self {
            session_id,
            created_at,
            cwd,
            model_info,
        }
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
        Self {
            data: SessionData::new(session_id, created_at, cwd, model_info),
            turns,
        }
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

    /// Record an input against the open input turn, opening a new one if the
    /// last turn was the assistant's.
    pub(crate) fn add_input(&mut self, entry: RecordedInput) -> TurnRecord {
        if let Some(RecordableTurn::Input { entries }) = self
            .turns
            .last_mut()
            .map(|turn| &mut turn.data.turn)
        {
            entries.push(entry);
            return self.turns.last().cloned().expect("turn to exist");
        }
        let parent_id = self.turns.last().map(|turn| turn.data.turn_id());
        let tr = TurnRecord::new(
            RecordableTurn::Input {
                entries: vec![entry],
            },
            parent_id,
        );
        self.turns.push(tr.clone());
        tr
    }

    /// Record an assistant turn. Only valid if the last turn was an input.
    pub(crate) fn add_assistant(
        &mut self,
        outcome: AssistantTurnOutcome,
    ) -> Result<TurnRecord, ()> {
        let previous_turn = self.turns.last().ok_or(())?;
        if !matches!(previous_turn.data.turn, RecordableTurn::Input { .. }) {
            return Err(());
        }
        let tr = TurnRecord::new(
            RecordableTurn::Assistant { outcome },
            Some(previous_turn.data.turn_id()),
        );
        self.turns.push(tr.clone());
        Ok(tr)
    }

    /// Fold the recorded turns into the driver's view of the conversation.
    /// An unsettled assistant turn only survives as the partial if nothing
    /// follows it; anything later means it was already resolved.
    pub fn restore_state(&self) -> RestoreState {
        let mut turns = Vec::new();
        let mut partial = None;
        for turn in &self.turns {
            partial = None;
            match turn.data.turn() {
                RecordableTurn::Input { entries } => turns.push(Turn::Input {
                    entries: entries
                        .iter()
                        .cloned()
                        .map(|entry| match entry {
                            RecordedInput::User(user) => InputEntry::User(user),
                            RecordedInput::Harness(message) => {
                                HarnessEntry::Message(message).into()
                            }
                            RecordedInput::ToolResult(result) => {
                                HarnessEntry::ToolResult(result.into()).into()
                            }
                        })
                        .collect(),
                }),
                RecordableTurn::Assistant { outcome } => match outcome {
                    AssistantTurnOutcome::Completed { response } => {
                        turns.push(Turn::Output(response.clone()))
                    }
                    AssistantTurnOutcome::Incomplete {
                        partial_response, ..
                    } => partial = partial_response.clone(),
                },
            }
        }
        RestoreState::new(turns, partial)
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
    fn new(
        turn_id: TurnId,
        timestamp: DateTime<Utc>,
        parent_id: Option<TurnId>,
        turn: RecordableTurn,
    ) -> Self {
        Self {
            turn_id,
            timestamp,
            parent_id,
            turn,
        }
    }
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecordableTurn {
    /// Input message from the harness. May be due to automatic - returning tool
    /// results, or just user sending message.
    Input { entries: Vec<RecordedInput> },
    /// Result emitted by the clanker.
    Assistant { outcome: AssistantTurnOutcome },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum RecordableEvent {
    ToolAuthorization {
        tool_call_id: ToolCallId,
        decision: ToolDecision,
        source: AuthorizationSource,
        reason: Option<String>,
    },
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
