/// Represents a user message in the conversation.
pub struct UserTurn {
    content: Vec<UserContent>
}

pub enum UserContent {
    Text(String),
    Image(ImageContent)
}

impl From<String> for UserContent {
    fn from(value: String) -> Self {
        UserContent::Text(value)
    }
}

pub enum ImageContent {
    Url(String),
    Base64(String)
}
