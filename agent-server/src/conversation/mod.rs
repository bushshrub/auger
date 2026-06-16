mod user;
mod clanker;

pub use user::{UserTurn, UserContent};
pub use clanker::ClankerTurn;


/// The system prompt
pub struct SystemMessage(Vec<String>);


pub enum Turn {
    User(UserTurn),
    ClankerTurn(ClankerTurn),
}

/// A conversation between the user and the clanker.
pub struct Conversation {
    system: SystemMessage,
    turns: Vec<Turn>,
}

impl Conversation {
    pub fn new(system: SystemMessage) -> Self {
        Self {
            system,
            turns: Vec::new(),
        }
    }

}
