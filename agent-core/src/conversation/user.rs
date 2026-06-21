use std::fmt::Display;
use serde::{Deserialize, Serialize};

/// Represents a user message in the conversation.
pub struct UserTurn {
    content: Vec<UserContent>
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UserContent {
    Text(String),
    Image(ImageContent)
}

impl Display for UserContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserContent::Text(text) => write!(f, "{}", text),
            UserContent::Image(image) => write!(f, "[Image: {}]", image)
        }
    }
}

impl From<String> for UserContent {
    fn from(value: String) -> Self {
        UserContent::Text(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ImageContent {
    Url(String),
    Base64(String)
}

impl Display for ImageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageContent::Url(url) => write!(f, "Image URL: {}", url),
            ImageContent::Base64(_) => write!(f, "Base64 Image Data")
        }
    }
}