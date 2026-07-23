use getset::Getters;
use crate::session::history::{EventRecord, SessionData, TurnData, TurnId};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TraceRecord<T, E> {
    Session(SessionHeader),
    Turn {
        #[serde(flatten)] record: T
    },
    Event {
        turn_id: TurnId,
        #[serde(flatten)] record: E,
    },
}

pub(crate) type OwnedTraceRecord = TraceRecord<TurnData, EventRecord>;
pub(crate) type TraceRecordRef<'a> = TraceRecord<&'a TurnData, &'a EventRecord>;


#[derive(Serialize, Deserialize, Debug, Clone, Getters)]
#[getset(get = "pub")]
pub struct SessionHeader {
    version: u32,
    #[serde(flatten)]
    data: SessionData
}

impl SessionHeader {
    pub(crate) fn new(data: SessionData) -> Self {
        Self {
            version: 1,
            data
        }
    }
}
