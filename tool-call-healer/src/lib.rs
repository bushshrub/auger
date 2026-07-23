use provider::CompletedLlmResponse;
use provider::ToolCallRequest;
use uuid::Uuid;

pub struct XmlStyleToolCallHealer;

impl XmlStyleToolCallHealer {
    pub fn heal(&self, mut response: CompletedLlmResponse) -> CompletedLlmResponse {
        let mut healed_tool_calls: Vec<ToolCallRequest> = Vec::new();
        let mut content = response.content.clone();

        while let Some(start) = content.find("<tool_call>") {
            let Some(end) = content.find("</tool_call>") else {
                break;
            };

            let tag_start = start;
            let tag_end = end + "</tool_call>".len();
            let inner = content[start + "<tool_call>".len()..end].trim();

            if let Some(tc) = parse_tool_call(inner) {
                healed_tool_calls.push(tc);
            }

            content.replace_range(tag_start..tag_end, "");
            content = content.trim().to_string();
        }

        if !healed_tool_calls.is_empty() {
            let existing = response.tool_calls.take().unwrap_or_default();
            let mut all = existing;
            all.extend(healed_tool_calls);
            response.tool_calls = Some(all);
            response.content = content;
        }

        response
    }
}

fn parse_tool_call(json: &str) -> Option<ToolCallRequest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let name = v["name"].as_str()?.to_string();
    let arguments = serde_json::to_string(&v["arguments"]).ok()?;
    Some(ToolCallRequest {
        id: Uuid::new_v4().to_string(),
        name,
        arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base_response(content: &str) -> CompletedLlmResponse {
        CompletedLlmResponse {
            content: content.to_string(),
            reasoning: None,
            tool_calls: None,
            usage: None,
            stop_reason: None,
        }
    }

    #[test]
    fn heals_single_tool_call() {
        let healer = XmlStyleToolCallHealer;
        let content =
            r#"Sure!<tool_call>{"name":"read_file","arguments":{"path":"/tmp/foo"}}</tool_call>"#;
        let result = healer.heal(base_response(content));

        let tcs = result.tool_calls.unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].name, "read_file");
        assert!(!tcs[0].id.is_empty());
        assert!(!result.content.contains("<tool_call>"));
    }

    #[test]
    fn heals_multiple_tool_calls() {
        let healer = XmlStyleToolCallHealer;
        let content = "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call> text \
                       <tool_call>{\"name\":\"b\",\"arguments\":{}}</tool_call>";
        let result = healer.heal(base_response(content));

        let tcs = result.tool_calls.unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0].name, "a");
        assert_eq!(tcs[1].name, "b");
    }

    #[test]
    fn no_tool_calls_passthrough() {
        let healer = XmlStyleToolCallHealer;
        let response = base_response("plain text");
        let result = healer.heal(response);
        assert_eq!(result.content, "plain text");
        assert!(result.tool_calls.is_none());
    }

    #[test]
    fn preserves_existing_tool_calls() {
        let healer = XmlStyleToolCallHealer;
        let mut response =
            base_response("<tool_call>{\"name\":\"b\",\"arguments\":{}}</tool_call>");
        response.tool_calls = Some(vec![ToolCallRequest {
            id: "existing-id".to_string(),
            name: "a".to_string(),
            arguments: "{}".to_string(),
        }]);
        let result = healer.heal(response);

        let tcs = result.tool_calls.unwrap();
        assert_eq!(tcs.len(), 2);
        assert_eq!(tcs[0].id, "existing-id");
        assert_eq!(tcs[1].name, "b");
    }
}
