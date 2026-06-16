pub(crate) struct CreateSessionRequest {
    pub(crate) model: Option<String>,
}

pub(crate) struct UserInputRequest {
    pub(crate) input: String
}

/// Whether the user approves or denies the tool use
pub(crate) enum ApproveRequest {
    // TODO: incorporate the user message
    Approve,
    Denied
}