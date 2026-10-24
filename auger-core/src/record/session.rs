use crate::record::turn::AssistantTurnOutcome;
use crate::record::turn::RecordableTurn;
use crate::record::turn::RecordedInput;
use crate::record::turn::TurnId;
use crate::record::turn::TurnRecord;
use auger_driver::HarnessEntry;
use auger_driver::InputEntry;
use auger_driver::RestoreState;
use auger_driver::Turn;
use chrono::DateTime;
use chrono::Utc;
use getset::CopyGetters;
use getset::Getters;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

/// ID of an auger session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SessionId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

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
        self.turns
            .iter_mut()
            .find(|tr| tr.data().turn_id() == *turn_id)
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
        if let Some(entries) = self
            .turns
            .last_mut()
            .and_then(TurnRecord::input_entries_mut)
        {
            entries.push(entry);
            return self.turns.last().cloned().expect("turn to exist");
        }
        let parent_id = self.turns.last().map(|turn| turn.data().turn_id());
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
        if !matches!(previous_turn.data().turn(), RecordableTurn::Input { .. }) {
            return Err(());
        }
        let tr = TurnRecord::new(
            RecordableTurn::Assistant { outcome },
            Some(previous_turn.data().turn_id()),
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
            match turn.data().turn() {
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
