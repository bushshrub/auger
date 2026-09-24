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
        let content = match result.outcome {
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
        provider::ToolResult {
            tool_call_id: result.tool_call_id.into(),
            // TODO: The provider should be updated to take a vec
            content: content
                .iter()
                .map(|d| match d {
                    ToolData::Text { text } => text.as_str(),
                })
                .collect::<Vec<_>>()
                .join(" "),
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolData {
    Text { text: String },
}
