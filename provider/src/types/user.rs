use crate::types::ToolResult;
use serde::Deserialize;
use serde::Serialize;

/// A block of a user message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserBlock {
    /// Text the user typed.
    Text(String),
    /// Text from the harness rather than the user, e.g. steering or a
    /// denial note. Adapters send it as user text marked by
    /// [`render_text`], since not every API has a developer role.
    Harness(String),
    /// The result of a tool call from the preceding assistant message.
    ToolResult(ToolResult),
}

/// The tool results of a user message, in order.
pub fn tool_results(blocks: &[UserBlock]) -> impl Iterator<Item = &ToolResult> {
    blocks.iter().filter_map(|block| match block {
        UserBlock::ToolResult(result) => Some(result),
        _ => None,
    })
}

/// The text of a user message as the model sees it. Harness text is wrapped
/// in `<system-reminder>` tags, with consecutive harness blocks sharing one
/// reminder. Parts are separated by blank lines; tool results are skipped.
pub fn render_text(blocks: &[UserBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut reminder: Vec<&str> = Vec::new();
    let flush = |reminder: &mut Vec<&str>, parts: &mut Vec<String>| {
        if !reminder.is_empty() {
            parts.push(format!(
                "<system-reminder>\n{}\n</system-reminder>",
                reminder.join("\n\n")
            ));
            reminder.clear();
        }
    };
    for block in blocks {
        match block {
            UserBlock::Text(text) => {
                flush(&mut reminder, &mut parts);
                if !text.is_empty() {
                    parts.push(text.clone());
                }
            }
            UserBlock::Harness(text) => reminder.push(text),
            UserBlock::ToolResult(_) => {}
        }
    }
    flush(&mut reminder, &mut parts);
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_harness_blocks_share_one_reminder() {
        let blocks = [
            UserBlock::Text("hi".to_string()),
            UserBlock::ToolResult(ToolResult::new("c".to_string(), "out".to_string())),
            UserBlock::Harness("a".to_string()),
            UserBlock::Harness("b".to_string()),
            UserBlock::Text("bye".to_string()),
        ];
        assert_eq!(
            render_text(&blocks),
            "hi\n\n<system-reminder>\na\n\nb\n</system-reminder>\n\nbye"
        );
    }
}
