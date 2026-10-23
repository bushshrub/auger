//! Tools a restored session called that the current tool set no longer has.

use auger_driver::Turn;

/// Names called in `turns` that `offered` does not contain, in first-call
/// order.
pub(crate) fn missing_tools(turns: &[Turn], offered: &[String]) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for turn in turns {
        let Turn::Output(response) = turn else {
            continue;
        };
        for call in response.tool_calls() {
            if !offered.contains(&call.name) && !missing.contains(&call.name) {
                missing.push(call.name);
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::{AssistantResponse, Block, ToolCallRequest};

    fn call(name: &str) -> Block {
        Block::ToolCall(ToolCallRequest {
            id: name.into(),
            name: name.into(),
            arguments: "{}".into(),
        })
    }

    #[test]
    fn lists_called_tools_missing_from_the_offer_once() {
        let turns = vec![
            Turn::Output(AssistantResponse::new(vec![call("read_file"), call("gone")]).unwrap()),
            Turn::Output(AssistantResponse::new(vec![call("gone"), call("mcp__x__y")]).unwrap()),
        ];
        let offered = vec!["read_file".to_string()];
        assert_eq!(missing_tools(&turns, &offered), ["gone", "mcp__x__y"]);
    }
}
