/// Represents a user message in the conversation.
pub struct UserTurn {
    content: Vec<UserContent>
}

pub enum UserContent {
    Text(String),
    Image(ImageContent)
}

pub enum ImageContent {
    Url(String),
    Base64(String)
}
