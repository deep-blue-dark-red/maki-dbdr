use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tracing::info;

use super::error::McpError;
use super::protocol::{
    CallToolResult, GetPromptResult, ListPage, PromptInfo, PromptsListResult, ToolInfo,
    ToolsListResult, initialize_params,
};

const METHOD_NOT_FOUND: i64 = -32601;
const REPEATED_CURSOR: &str = "repeated pagination cursor";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait McpTransport: Send + Sync {
    fn send_request<'a>(
        &'a self,
        method: &'a str,
        params: Option<Value>,
    ) -> BoxFuture<'a, Result<Value, McpError>>;
    fn send_notification<'a>(
        &'a self,
        method: &'a str,
        params: Option<Value>,
    ) -> BoxFuture<'a, Result<(), McpError>>;
    fn shutdown<'a>(&'a self) -> BoxFuture<'a, ()>;
    fn server_name(&self) -> &Arc<str>;
    fn transport_kind(&self) -> &'static str;
    fn child_pids(&self) -> Vec<u32> {
        Vec::new()
    }
}

fn invalid_response(name: &Arc<str>, e: impl std::fmt::Display) -> McpError {
    McpError::InvalidResponse {
        server: (**name).into(),
        reason: e.to_string(),
    }
}

pub struct ServerCapabilities {
    pub tools: bool,
    pub prompts: bool,
}

impl ServerCapabilities {
    fn parse(result: &Value) -> Self {
        Self {
            tools: result["capabilities"]["tools"].is_object(),
            prompts: result["capabilities"]["prompts"].is_object(),
        }
    }
}

pub async fn initialize(transport: &dyn McpTransport) -> Result<ServerCapabilities, McpError> {
    let params = initialize_params();
    let result = transport.send_request("initialize", Some(params)).await?;
    transport
        .send_notification("notifications/initialized", None)
        .await?;
    Ok(ServerCapabilities::parse(&result))
}

pub async fn list_tools(transport: &dyn McpTransport) -> Result<Vec<ToolInfo>, McpError> {
    list_pages(transport, "tools/list", |list: ToolsListResult| list.tools).await
}

pub async fn list_prompts(transport: &dyn McpTransport) -> Result<Vec<PromptInfo>, McpError> {
    match list_pages(transport, "prompts/list", |list: PromptsListResult| {
        list.prompts
    })
    .await
    {
        Ok(prompts) => Ok(prompts),
        Err(McpError::RpcError { code, .. }) if code == METHOD_NOT_FOUND => Ok(vec![]),
        Err(e) => Err(e),
    }
}

async fn list_pages<T: DeserializeOwned, I>(
    transport: &dyn McpTransport,
    method: &str,
    items: impl Fn(T) -> Vec<I>,
) -> Result<Vec<I>, McpError> {
    let mut collected = Vec::new();
    let mut cursor = None;
    let mut seen = HashSet::new();
    loop {
        let params = cursor.map(|cursor| json!({ "cursor": cursor }));
        let result = transport.send_request(method, params).await?;
        let page: ListPage<T> = serde_json::from_value(result)
            .map_err(|e| invalid_response(transport.server_name(), e))?;
        collected.extend(items(page.items));
        let Some(next) = page.next_cursor else {
            return Ok(collected);
        };
        if !seen.insert(next.clone()) {
            return Err(invalid_response(transport.server_name(), REPEATED_CURSOR));
        }
        cursor = Some(next);
    }
}

pub async fn get_prompt(
    transport: &dyn McpTransport,
    name: &str,
    arguments: &HashMap<String, String>,
) -> Result<Vec<super::protocol::PromptMessage>, McpError> {
    let params = serde_json::json!({ "name": name, "arguments": arguments });
    let result = transport.send_request("prompts/get", Some(params)).await?;
    let parsed: GetPromptResult =
        serde_json::from_value(result).map_err(|e| invalid_response(transport.server_name(), e))?;
    Ok(parsed.messages)
}

pub async fn call_tool(
    transport: &dyn McpTransport,
    tool_name: &str,
    args: &Value,
) -> Result<String, McpError> {
    let server = &**transport.server_name();
    let start = Instant::now();
    let params = serde_json::json!({
        "name": tool_name,
        "arguments": args,
    });
    let result = transport.send_request("tools/call", Some(params)).await?;
    let call_result: CallToolResult =
        serde_json::from_value(result).map_err(|e| invalid_response(transport.server_name(), e))?;

    let text = call_result.joined_text();

    if call_result.is_error {
        return Err(McpError::RpcError {
            server: (**transport.server_name()).into(),
            code: -1,
            message: text,
        });
    }

    info!(
        server,
        tool = tool_name,
        duration_ms = start.elapsed().as_millis() as u64,
        "MCP tools/call response"
    );
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;
    use serde_json::json;
    use test_case::test_case;

    const SERVER: &str = "pagination-test";
    const FIRST: &str = "first";
    const LAST: &str = "last";
    const CURSOR_ONE: &str = "page-2";
    const CURSOR_TWO: &str = "page-3";

    struct StubTransport {
        name: Arc<str>,
        responses: Mutex<VecDeque<Result<Value, McpError>>>,
        requests: Mutex<Vec<(String, Option<Value>)>>,
    }

    impl StubTransport {
        fn new(responses: impl IntoIterator<Item = Result<Value, McpError>>) -> Self {
            Self {
                name: Arc::from(SERVER),
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Mutex::default(),
            }
        }
    }

    impl McpTransport for StubTransport {
        fn send_request<'a>(
            &'a self,
            method: &'a str,
            params: Option<Value>,
        ) -> BoxFuture<'a, Result<Value, McpError>> {
            Box::pin(async move {
                self.requests
                    .lock()
                    .unwrap()
                    .push((method.to_string(), params));
                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected request")
            })
        }

        fn send_notification<'a>(
            &'a self,
            _: &'a str,
            _: Option<Value>,
        ) -> BoxFuture<'a, Result<(), McpError>> {
            Box::pin(async { Ok(()) })
        }

        fn shutdown<'a>(&'a self) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }

        fn server_name(&self) -> &Arc<str> {
            &self.name
        }
        fn transport_kind(&self) -> &'static str {
            "stub"
        }
    }

    async fn listed_names(
        transport: &dyn McpTransport,
        method: &str,
    ) -> Result<Vec<String>, McpError> {
        if method == "tools/list" {
            list_tools(transport)
                .await
                .map(|items| items.into_iter().map(|item| item.name).collect())
        } else {
            list_prompts(transport)
                .await
                .map(|items| items.into_iter().map(|item| item.name).collect())
        }
    }

    #[test_case("tools/list", "tools"; "tools")]
    #[test_case("prompts/list", "prompts"; "prompts")]
    fn discovery_follows_cursors_through_empty_pages(method: &str, field: &str) {
        smol::block_on(async {
            let transport = StubTransport::new([
                Ok(json!({(field): [{"name": FIRST}], "nextCursor": CURSOR_ONE})),
                Ok(json!({(field): [], "nextCursor": CURSOR_TWO})),
                Ok(json!({(field): [{"name": LAST}]})),
            ]);
            assert_eq!(
                listed_names(&transport, method).await.unwrap(),
                [FIRST, LAST]
            );
            assert_eq!(
                *transport.requests.lock().unwrap(),
                [
                    (method.to_string(), None),
                    (method.to_string(), Some(json!({"cursor": CURSOR_ONE}))),
                    (method.to_string(), Some(json!({"cursor": CURSOR_TWO}))),
                ]
            );
        });
    }

    #[test_case("tools/list", "tools"; "tools")]
    #[test_case("prompts/list", "prompts"; "prompts")]
    fn discovery_rejects_repeated_cursors(method: &str, field: &str) {
        smol::block_on(async {
            let transport = StubTransport::new([
                Ok(json!({(field): [], "nextCursor": CURSOR_ONE})),
                Ok(json!({(field): [], "nextCursor": CURSOR_ONE})),
            ]);
            let error = listed_names(&transport, method).await.unwrap_err();
            assert!(
                matches!(error, McpError::InvalidResponse { reason, .. } if reason == REPEATED_CURSOR)
            );
        });
    }

    #[test]
    fn missing_prompts_endpoint_is_optional() {
        smol::block_on(async {
            let transport = StubTransport::new([Err(McpError::RpcError {
                server: SERVER.into(),
                code: METHOD_NOT_FOUND,
                message: "not found".into(),
            })]);
            assert!(list_prompts(&transport).await.unwrap().is_empty());
        });
    }

    #[test_case(json!({"capabilities": {"tools": {}, "prompts": {}}}), true, true ; "both")]
    #[test_case(json!({"capabilities": {"tools": {"listChanged": false}}}), true, false ; "tools_only")]
    #[test_case(json!({"capabilities": {"prompts": {}}}), false, true ; "prompts_only")]
    #[test_case(json!({}), false, false ; "no_capabilities")]
    fn parses_capabilities(result: Value, tools: bool, prompts: bool) {
        let caps = ServerCapabilities::parse(&result);
        assert_eq!((caps.tools, caps.prompts), (tools, prompts));
    }
}
