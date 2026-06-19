use serde::{Deserialize, Serialize};

/// Represents a user message in the conversation.
pub struct UserTurn {
    content: Vec<UserContent>
}

#[derive(Clone, Serialize, Deserialize)]
pub enum UserContent {
    Text(String),
    Image(ImageContent)
}

impl From<String> for UserContent {
    fn from(value: String) -> Self {
        UserContent::Text(value)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ImageContent {
    Url(String),
    Base64(String)
}
