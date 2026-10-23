use auger_driver::ToolCallId;
use getset::CloneGetters;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize, Debug, Clone, CloneGetters)]
pub struct ToolCallResult {
    #[getset(get_clone = "pub")]
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) outcome: ToolOutcome,
}

impl From<ToolCallResult> for provider::ToolResult {
    fn from(result: ToolCallResult) -> Self {
        provider::ToolResult {
            tool_call_id: result.tool_call_id.into(),
            content: result.outcome.into_text(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    Success { content: Vec<ToolData> },
    Error { error: Vec<ToolData> },
    Denied { reason: Option<String> },
    Interrupted,
}

impl ToolOutcome {
    pub(crate) fn success(text: impl Into<String>) -> Self {
        Self::Success {
            content: vec![ToolData::Text { text: text.into() }],
        }
    }

    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self::Error {
            error: vec![ToolData::Text { text: text.into() }],
        }
    }

    /// The text the model sees for this outcome.
    pub(crate) fn into_text(self) -> String {
        let content = match self {
            ToolOutcome::Success { content } => content,
            ToolOutcome::Error { error } => error,
            ToolOutcome::Denied { reason } => match reason {
                Some(r) => vec![ToolData::Text {
                    text: format!("Tool call was denied: {}", r),
                }],
                None => vec![ToolData::Text {
                    text: "Tool call was denied. Do not attempt to make the same tool call.".into(),
                }],
            },
            ToolOutcome::Interrupted => {
                vec![ToolData::Text {
                    text: "Tool call was interrupted. Do not attempt to make the same tool call."
                        .into(),
                }]
            }
        };
        // TODO: The provider should be updated to take a vec
        content
            .iter()
            .map(|d| match d {
                ToolData::Text { text } => text.as_str(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolData {
    Text { text: String },
}
