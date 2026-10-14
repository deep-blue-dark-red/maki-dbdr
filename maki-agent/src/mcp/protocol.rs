use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

#[derive(Serialize)]
pub struct JsonRpcRequest<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> JsonRpcRequest<'a> {
    pub fn new(id: u64, method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method,
            params,
        }
    }
}

#[derive(Serialize)]
pub struct JsonRpcNotification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl<'a> JsonRpcNotification<'a> {
    pub fn new(method: &'a str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}

#[derive(Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

pub fn initialize_params() -> Value {
    serde_json::json!({
        "protocolVersion": LATEST_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": "maki",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

#[derive(Deserialize)]
pub struct ToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<ToolInfo>,
}

#[derive(Deserialize)]
pub struct ListPage<T> {
    #[serde(flatten)]
    pub items: T,
    #[serde(default, rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CallToolContent {
    Text {
        text: String,
    },
    Resource {
        resource: EmbeddedResource,
    },
    ResourceLink {
        uri: String,
        name: String,
    },
    Image {
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Audio {
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(other)]
    Unsupported,
}

#[derive(Deserialize)]
pub struct EmbeddedResource {
    pub uri: String,
    pub text: Option<String>,
}

impl CallToolContent {
    fn text(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Resource { resource } => resource
                .text
                .clone()
                .unwrap_or_else(|| format!("[Unsupported MCP binary resource: {}]", resource.uri)),
            Self::ResourceLink { uri, name } => format!("{name}: {uri}"),
            Self::Image { mime_type } => format!("[Unsupported MCP image content: {mime_type}]"),
            Self::Audio { mime_type } => format!("[Unsupported MCP audio content: {mime_type}]"),
            Self::Unsupported => "[Unsupported MCP content type]".to_string(),
        }
    }
}

#[derive(Deserialize)]
pub struct CallToolResult {
    pub content: Vec<CallToolContent>,
    #[serde(default, rename = "structuredContent")]
    pub structured_content: Option<Value>,
    #[serde(default, rename = "isError")]
    pub is_error: bool,
}

impl CallToolResult {
    pub fn joined_text(&self) -> String {
        let text = self
            .content
            .iter()
            .map(CallToolContent::text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            self.structured_content
                .as_ref()
                .map_or(text, Value::to_string)
        } else {
            text
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct PromptArgument {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Deserialize)]
pub struct PromptInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub arguments: Vec<PromptArgument>,
}

#[derive(Deserialize)]
pub struct PromptsListResult {
    pub prompts: Vec<PromptInfo>,
}

#[derive(Deserialize)]
pub struct PromptMessageContent {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    #[default]
    User,
    Assistant,
}

#[derive(Deserialize)]
pub struct PromptMessage {
    #[serde(default)]
    pub role: PromptRole,
    pub content: PromptMessageContent,
}

#[derive(Deserialize)]
pub struct GetPromptResult {
    pub messages: Vec<PromptMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use test_case::test_case;

    const RESOURCE_TEXT: &str = "critical answer";
    const RESOURCE_URI: &str = "file:///answer.txt";
    const RESOURCE_LINK: &str = "answer: file:///answer.txt";
    const BINARY_RESOURCE: &str = "[Unsupported MCP binary resource: file:///answer.txt]";
    const IMAGE_CONTENT: &str = "[Unsupported MCP image content: image/png]";
    const AUDIO_CONTENT: &str = "[Unsupported MCP audio content: audio/wav]";
    const UNKNOWN_CONTENT: &str = "[Unsupported MCP content type]";

    #[test_case(json!({"type": "resource", "resource": {"uri": RESOURCE_URI, "text": RESOURCE_TEXT}}), RESOURCE_TEXT; "embedded_text")]
    #[test_case(json!({"type": "resource", "resource": {"uri": RESOURCE_URI, "blob": "YQ=="}}), BINARY_RESOURCE; "embedded_binary")]
    #[test_case(json!({"type": "resource_link", "uri": RESOURCE_URI, "name": "answer"}), RESOURCE_LINK; "resource_link")]
    #[test_case(json!({"type": "image", "mimeType": "image/png", "data": "YQ=="}), IMAGE_CONTENT; "image")]
    #[test_case(json!({"type": "audio", "mimeType": "audio/wav", "data": "YQ=="}), AUDIO_CONTENT; "audio")]
    #[test_case(json!({"type": "future_content"}), UNKNOWN_CONTENT; "unknown")]
    fn tool_content_is_not_silently_discarded(content: Value, expected: &str) {
        let result: CallToolResult = serde_json::from_value(json!({"content": [content]})).unwrap();
        assert_eq!(result.joined_text(), expected);
    }

    #[test_case(json!([]), "{\"answer\":42}"; "structured_only")]
    #[test_case(json!([{"type": "text", "text": ""}, {"type": "text", "text": ""}]), "{\"answer\":42}"; "empty_text_blocks")]
    #[test_case(json!([{"type": "text", "text": "answer: 42"}]), "answer: 42"; "text_not_duplicated")]
    fn structured_content_fills_empty_text(content: Value, expected: &str) {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": content,
            "structuredContent": {"answer": 42}
        }))
        .unwrap();
        assert_eq!(result.joined_text(), expected);
    }

    #[test]
    fn request_skips_none_params() {
        let with =
            serde_json::to_value(JsonRpcRequest::new(1, "init", Some(json!({"k": 1})))).unwrap();
        assert_eq!(with["params"]["k"], 1);

        let without = serde_json::to_value(JsonRpcRequest::new(2, "tools/list", None)).unwrap();
        assert!(without.get("params").is_none());
    }

    #[test]
    fn tool_info_honours_input_schema_rename() {
        let raw = json!({"tools": [{"name": "read_file", "description": "Read a file", "inputSchema": {"type": "object"}}]});
        let result: ToolsListResult = serde_json::from_value(raw).unwrap();
        assert_eq!(result.tools[0].name, "read_file");
        assert_eq!(result.tools[0].input_schema["type"], "object");
    }

    #[test]
    fn call_tool_result_honours_is_error_rename() {
        let raw = json!({"content": [{"type": "text", "text": "hello"}], "isError": true});
        let result: CallToolResult = serde_json::from_value(raw).unwrap();
        assert!(result.is_error);
        assert_eq!(result.joined_text(), "hello");
    }

    #[test]
    fn prompts_list_result_deserializes() {
        let raw = json!({
            "prompts": [{
                "name": "code-review",
                "description": "Review code changes",
                "arguments": [
                    {"name": "diff", "description": "The diff to review", "required": true},
                    {"name": "style", "required": false}
                ]
            }]
        });
        let result: PromptsListResult = serde_json::from_value(raw).unwrap();
        assert_eq!(result.prompts.len(), 1);
        assert_eq!(result.prompts[0].name, "code-review");
        assert_eq!(
            result.prompts[0].description.as_deref(),
            Some("Review code changes")
        );
        assert_eq!(result.prompts[0].arguments.len(), 2);
        assert!(result.prompts[0].arguments[0].required);
        assert!(!result.prompts[0].arguments[1].required);
    }

    #[test]
    fn prompts_list_result_defaults() {
        let raw = json!({"prompts": [{"name": "simple"}]});
        let result: PromptsListResult = serde_json::from_value(raw).unwrap();
        assert!(result.prompts[0].description.is_none());
        assert!(result.prompts[0].arguments.is_empty());
    }

    #[test]
    fn get_prompt_result_deserializes() {
        let raw = json!({
            "messages": [
                {"role": "user", "content": {"text": "Review this code"}},
                {"role": "assistant", "content": {"text": "I'll review it"}}
            ]
        });
        let result: GetPromptResult = serde_json::from_value(raw).unwrap();
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.messages[0].role, PromptRole::User);
        assert_eq!(
            result.messages[0].content.text.as_deref(),
            Some("Review this code")
        );
        assert_eq!(result.messages[1].role, PromptRole::Assistant);
    }
}
