use serde::{Deserialize, Serialize};

/// Data returned by a tool.
///
/// Currently only text is supported. Images will be added later.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolData {
    Text(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolDecision {
    Approved,
    Denied,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolCallStatus {
    Success,
    Denied,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthorizationSource {
    User,
    Policy,
}
