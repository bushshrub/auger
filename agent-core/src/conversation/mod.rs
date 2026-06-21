mod user;
mod clanker;

pub use user::{UserTurn, UserContent};
pub use clanker::ClankerTurn;


/// The system prompt
pub(crate) struct SystemMessage(Vec<String>);

impl From<String> for SystemMessage {
    fn from(value: String) -> Self {
        SystemMessage(vec![value])
    }
}


pub(crate) enum Turn {
    User(UserTurn),
    ClankerTurn(ClankerTurn),
}

/// A conversation between the user and the clanker.
pub(crate) struct Conversation {
    system: SystemMessage,
    turns: Vec<Turn>,
}

impl Conversation {
    pub(crate) fn new(system: SystemMessage) -> Self {
        Self {
            system,
            turns: Vec::new(),
        }
    }

}
