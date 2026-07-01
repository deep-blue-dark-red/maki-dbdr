use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_lite::StreamExt;
use futures_lite::io::AsyncBufRead;
use isahc::config::Configurable;
use serde::Deserialize;
use tracing::debug;

use crate::AgentError;

pub(crate) mod anthropic;
pub(crate) mod copilot;
pub mod custom;
pub(crate) mod deepseek;
pub mod dynamic;
pub(crate) mod google;
pub(crate) mod llama_cpp;
pub(crate) mod local;
pub(crate) mod mistral;
pub(crate) mod ollama;
pub(crate) mod openai;
pub(crate) mod openai_compat;
pub(crate) mod openrouter;
pub(crate) mod synthetic;
pub(crate) mod tensorx;
pub(crate) mod zai;

const LOW_SPEED_BYTES_PER_SEC: u32 = 1;

pub(crate) fn user_agent() -> &'static str {
    concat!(
        "maki/v",
        env!("CARGO_PKG_VERSION"),
        "-g",
        env!("GIT_SHORT_HASH")
    )
}

#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub stream: Duration,
    pub low_speed: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            stream: Duration::from_secs(300),
            low_speed: Duration::from_secs(30),
        }
    }
}

#[derive(Clone)]
pub struct ResolvedAuth {
    pub base_url: Option<String>,
    pub headers: Vec<(String, String)>,
}

impl ResolvedAuth {
    pub fn bearer(api_key: &str) -> Self {
        Self {
            base_url: None,
            headers: vec![("authorization".into(), format!("Bearer {api_key}"))],
        }
    }
}

pub(crate) fn with_prefix<'a>(
    prefix: &Option<String>,
    system: &'a str,
    buf: &'a mut String,
) -> &'a str {
    match prefix {
        Some(p) => {
            *buf = format!("{p}\n\n{system}");
            buf
        }
        None => system,
    }
}

pub(crate) fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

#[derive(Deserialize)]
pub(crate) struct SseErrorPayload {
    pub error: SseErrorDetail,
}

#[derive(Deserialize)]
pub(crate) struct SseErrorDetail {
    #[serde(default)]
    pub r#type: String,
    pub message: String,
}

impl SseErrorPayload {
    pub fn into_agent_error(self) -> AgentError {
        let status = match self.error.r#type.as_str() {
            "overloaded_error" => 529,
            "api_error" | "server_error" => 500,
            "rate_limit_error" | "rate_limit_exceeded" | "tokens" => 429,
            "request_too_large" => 413,
            "not_found_error" => 404,
            "permission_error" => 403,
            "billing_error" | "insufficient_quota" => 402,
            "authentication_error" | "invalid_api_key" => 401,
            _ => 400,
        };
        AgentError::Api {
            status,
            message: self.error.message,
        }
    }
}

pub(crate) async fn next_sse_line<R: AsyncBufRead + Unpin>(
    lines: &mut futures_lite::io::Lines<R>,
    deadline: &mut Instant,
    stream_timeout: Duration,
) -> Result<Option<String>, AgentError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let result = futures_lite::future::or(
        async { lines.next().await.transpose().map_err(AgentError::from) },
        async {
            smol::Timer::after(remaining).await;
            Err(AgentError::Timeout {
                secs: stream_timeout.as_secs(),
            })
        },
    )
    .await;
    if let Ok(Some(_)) = &result {
        *deadline = Instant::now() + stream_timeout;
    }
    result
}

pub(crate) fn http_client(timeouts: Timeouts) -> isahc::HttpClient {
    isahc::HttpClient::builder()
        .connect_timeout(timeouts.connect)
        .low_speed_timeout(LOW_SPEED_BYTES_PER_SEC, timeouts.low_speed)
        .build()
        .expect("failed to build HTTP client")
}

#[derive(serde::Serialize)]
struct RequestLog {
    timestamp: String,
    #[serde(rename = "type")]
    log_type: &'static str,
    method: String,
    uri: String,
    body: serde_json::Value,
}

#[derive(serde::Serialize)]
struct ResponseLog {
    timestamp: String,
    #[serde(rename = "type")]
    log_type: &'static str,
    status: u16,
    body: serde_json::Value,
}

fn format_timestamp(ts: jiff::Timestamp) -> String {
    let s = ts.to_string();
    if s.len() >= 19 {
        s[..19].replace('T', " ")
    } else {
        s.replace('T', " ")
    }
}

fn parse_body(body: &str) -> serde_json::Value {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        val
    } else {
        serde_json::Value::String(body.to_string())
    }
}

fn simplify_tools(mut body: serde_json::Value) -> serde_json::Value {
    if let Some(tools) = body.as_object_mut()
        .and_then(|obj| obj.get_mut("tools"))
        .and_then(|t| t.as_array_mut())
    {
        for tool in tools {
            if let Some(func) = tool.as_object_mut()
                .and_then(|tool_obj| tool_obj.get_mut("function"))
                .and_then(|f| f.as_object_mut())
            {
                func.remove("description");
                func.remove("parameters");
            }
        }
    }
    body
}

fn clean_base64_images(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            if let Some(url_val) = map.get_mut("image_url")
                .and_then(|v| v.as_object_mut())
                .and_then(|m| m.get_mut("url"))
            {
                let is_image_data = url_val.as_str()
                    .is_some_and(|s| s.starts_with("data:image/"));
                if is_image_data {
                    let url_str = url_val.as_str().unwrap();
                    if let Some(comma_idx) = url_str.find(";base64,") {
                        let mime = &url_str[11..comma_idx];
                        let base64_part = &url_str[comma_idx + 8..];
                        let size_kb = base64_part.len() * 3 / 4 / 1024;
                        *url_val = serde_json::Value::String(format!("[base64 image: {}KB {}]", size_kb, mime));
                    }
                }
            }

            if let Some(source_val) = map.get_mut("source").and_then(|v| v.as_object_mut()) {
                let is_base64 = source_val.get("type").and_then(|t| t.as_str()) == Some("base64");
                if is_base64 {
                    let media_type = source_val.get("media_type")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    if let Some(data_val) = source_val.get_mut("data").filter(|d| d.is_string()) {
                        let data_str = data_val.as_str().unwrap();
                        let size_kb = data_str.len() * 3 / 4 / 1024;
                        *data_val = serde_json::Value::String(format!("[base64 image: {}KB {}]", size_kb, media_type));
                    }
                }
            }

            for (_, v) in map.iter_mut() {
                clean_base64_images(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                clean_base64_images(v);
            }
        }
        _ => {}
    }
}

fn clean_response_body(body_str: &str, content_type: Option<&str>) -> serde_json::Value {
    let is_sse = content_type.is_some_and(|ct| ct.contains("event-stream"))
        || body_str.contains("data: ");

    if is_sse {
        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut final_usage = serde_json::Value::Null;
        let mut model = String::new();

        for line in body_str.lines() {
            let line = line.trim();
            if let Some(data_str) = line.strip_prefix("data: ") {
                if data_str.trim() == "[DONE]" {
                    continue;
                }
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(data_str) {
                    if let Some(m) = val["model"].as_str().filter(|_| model.is_empty()) {
                        model = m.to_string();
                    }
                    if let Some(delta) = val["choices"].as_array()
                        .and_then(|arr| arr.first())
                        .and_then(|c| c.get("delta"))
                    {
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                            assistant_text.push_str(content);
                        }
                        if let Some(reasoning) = delta.get("reasoning").and_then(|r| r.as_str()) {
                            reasoning_text.push_str(reasoning);
                        } else if let Some(reasoning) = delta.get("reasoning_content").and_then(|r| r.as_str()) {
                            reasoning_text.push_str(reasoning);
                        }
                    }
                    if val.get("usage").is_some() {
                        final_usage = val["usage"].clone();
                    }
                }
            }
        }

        let mut res_map = serde_json::Map::new();
        res_map.insert("stream_reconstructed".to_string(), serde_json::Value::Bool(true));
        if !model.is_empty() {
            res_map.insert("model".to_string(), serde_json::Value::String(model));
        }
        res_map.insert("content".to_string(), serde_json::Value::String(assistant_text));
        if !reasoning_text.is_empty() {
            res_map.insert("reasoning".to_string(), serde_json::Value::String(reasoning_text));
        }
        if !final_usage.is_null() {
            res_map.insert("usage".to_string(), final_usage);
        }
        serde_json::Value::Object(res_map)
    } else {
        parse_body(body_str)
    }
}

fn write_log_line(path: &std::path::Path, line: &str) {
    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    if let Ok(mut encoder) = file.and_then(|f| zstd::stream::Encoder::new(f, 3)) {
        let _ = writeln!(encoder, "{}", line);
        let _ = encoder.finish();
    }
}

fn is_chat_completion_request(method: &str, uri: &str) -> bool {
    method == "POST" && (
        uri.contains("/chat/completions")
        || uri.contains("/messages")
        || uri.contains("/generateContent")
        || uri.contains("/streamGenerateContent")
        || uri.contains("/invoke")
    )
}

pub(crate) async fn send_request(
    client: &isahc::HttpClient,
    request: isahc::Request<Vec<u8>>,
) -> Result<isahc::Response<isahc::AsyncBody>, AgentError> {
    use std::sync::atomic::Ordering;

    let method = request.method().to_string();
    let uri = request.uri().to_string();

    if !maki_config::LOG_API.load(Ordering::Relaxed) || !is_chat_completion_request(&method, &uri) {
        let (parts, body) = request.into_parts();
        let async_body = isahc::AsyncBody::from(body);
        let req = isahc::Request::from_parts(parts, async_body);
        return client.send_async(req).await.map_err(Into::into);
    }

    let logs_dir = match maki_storage::paths::logs_dir() {
        Ok(dir) => dir,
        Err(_) => {
            let (parts, body) = request.into_parts();
            let async_body = isahc::AsyncBody::from(body);
            let req = isahc::Request::from_parts(parts, async_body);
            return client.send_async(req).await.map_err(Into::into);
        }
    };
    let now = jiff::Timestamp::now();
    let yyyymmdd = now.to_string()[..10].replace('-', "");
    let session_name = maki_config::CURRENT_SESSION_NAME.lock().ok()
        .and_then(|guard| guard.clone())
        .filter(|name| name != "Main" && !name.is_empty());
    let session_id = maki_config::CURRENT_SESSION_ID.lock().ok()
        .and_then(|guard| guard.clone())
        .filter(|id| !id.is_empty());
    let name_or_id = if let Some(ref name) = session_name {
        let mut sanitized = String::new();
        for c in name.chars() {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                sanitized.push(c);
            } else if c.is_whitespace() {
                sanitized.push('_');
            }
        }
        if sanitized.is_empty() {
            session_id.unwrap_or_else(|| "unknown".to_string())
        } else {
            sanitized
        }
    } else {
        session_id.unwrap_or_else(|| "unknown".to_string())
    };
    let filename = format!("{}-{}.log.zst", yyyymmdd, name_or_id);
    let file_path = logs_dir.join(filename);

    let timestamp = format_timestamp(jiff::Timestamp::now());
    let is_first_request = !file_path.exists();
    let req_body_str = String::from_utf8_lossy(request.body());
    let mut req_body_val = parse_body(&req_body_str);
    if !is_first_request {
        req_body_val = simplify_tools(req_body_val);
    }
    clean_base64_images(&mut req_body_val);

    let req_log = RequestLog {
        timestamp,
        log_type: "request",
        method,
        uri,
        body: req_body_val,
    };
    if let Ok(log_line) = serde_json::to_string(&req_log) {
        write_log_line(&file_path, &log_line);
    }

    let (parts, body) = request.into_parts();
    let async_body = isahc::AsyncBody::from(body);
    let req = isahc::Request::from_parts(parts, async_body);

    let response = client.send_async(req).await?;

    let status = response.status();
    let content_type = response.headers().get("content-type").and_then(|h| h.to_str().ok().map(|s| s.to_string()));

    let (res_parts, res_body) = response.into_parts();
    let logged_body = LoggingBody {
        inner: res_body,
        file_path,
        status,
        content_type,
        accumulated_body: Vec::new(),
        flushed: false,
    };

    Ok(isahc::Response::from_parts(res_parts, isahc::AsyncBody::from_reader(logged_body)))
}

struct LoggingBody<R> {
    inner: R,
    file_path: std::path::PathBuf,
    status: isahc::http::StatusCode,
    content_type: Option<String>,
    accumulated_body: Vec<u8>,
    flushed: bool,
}

impl<R> LoggingBody<R> {
    fn flush_log(&mut self) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        let response_body_str = String::from_utf8_lossy(&self.accumulated_body);
        let res_body_val = clean_response_body(&response_body_str, self.content_type.as_deref());
        let res_timestamp = format_timestamp(jiff::Timestamp::now());

        let res_log = ResponseLog {
            timestamp: res_timestamp,
            log_type: "response",
            status: self.status.as_u16(),
            body: res_body_val,
        };
        if let Ok(log_line) = serde_json::to_string(&res_log) {
            write_log_line(&self.file_path, &log_line);
        }
    }
}

impl<R: futures_lite::io::AsyncRead + Unpin> futures_lite::io::AsyncRead for LoggingBody<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let res = std::pin::Pin::new(&mut this.inner).poll_read(cx, buf);
        match &res {
            std::task::Poll::Ready(Ok(0)) => {
                this.flush_log();
            }
            std::task::Poll::Ready(Ok(n)) => {
                this.accumulated_body.extend_from_slice(&buf[..*n]);
            }
            _ => {}
        }
        res
    }
}

impl<R> Drop for LoggingBody<R> {
    fn drop(&mut self) {
        self.flush_log();
    }
}


#[derive(Clone, Debug)]
pub struct KeyPool {
    keys: Arc<Vec<String>>,
    index: Arc<AtomicUsize>,
}

impl KeyPool {
    pub fn from_env(env_var: &str) -> Result<Self, AgentError> {
        let raw = std::env::var(env_var).map_err(|_| AgentError::Config {
            message: format!("{env_var} not set"),
        })?;
        let keys: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if keys.is_empty() {
            return Err(AgentError::Config {
                message: format!("{env_var} is empty"),
            });
        }
        Ok(Self {
            keys: Arc::new(keys),
            index: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn resolve(slug: &str, env_var: &str) -> Result<Self, AgentError> {
        if let Ok(pool) = Self::from_env(env_var) {
            debug!(slug, keys = pool.len(), "resolved API key from env");
            return Ok(pool);
        }
        if let Some(key) = Self::key_from_file(slug) {
            debug!(slug, "resolved API key from saved credentials");
            return Ok(Self::from_keys(vec![key]));
        }
        if let Some(key) = Self::key_from_config(slug) {
            debug!(slug, "resolved API key from providers.toml");
            return Ok(Self::from_keys(vec![key]));
        }
        Err(AgentError::Config {
            message: format!(
                "{env_var} not set and no saved credentials for '{slug}' — run `maki auth login {slug}`"
            ),
        })
    }

    fn key_from_file(slug: &str) -> Option<String> {
        let dir = maki_storage::StateDir::resolve().ok()?;
        maki_storage::auth::load_provider_credentials(&dir, slug).map(|c| c.api_key)
    }

    fn key_from_config(slug: &str) -> Option<String> {
        maki_config::providers::ProvidersConfig::load()
            .get(slug)
            .and_then(|d| d.api_key.clone())
    }

    pub(crate) fn from_keys(keys: Vec<String>) -> Self {
        Self {
            keys: Arc::new(keys),
            index: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn current(&self) -> &str {
        &self.keys[self.index.load(Ordering::Relaxed) % self.keys.len()]
    }

    pub fn rotate(&self) -> bool {
        if self.keys.len() <= 1 {
            return false;
        }
        self.index.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub fn rotate_auth(
        &self,
        auth: &Mutex<ResolvedAuth>,
        build: impl FnOnce(&str) -> ResolvedAuth,
    ) -> bool {
        if !self.rotate() {
            return false;
        }
        *auth.lock().unwrap() = build(self.current());
        true
    }

    pub fn rotate_headers(
        &self,
        auth: &Mutex<ResolvedAuth>,
        build: impl FnOnce(&str) -> Vec<(String, String)>,
    ) -> bool {
        if !self.rotate() {
            return false;
        }
        auth.lock().unwrap().headers = build(self.current());
        true
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_lite::io::AsyncBufReadExt;
    use test_case::test_case;

    #[test_case("a b", "a%20b" ; "space")]
    #[test_case("a:b", "a%3Ab" ; "colon")]
    #[test_case("abc", "abc"   ; "passthrough")]
    fn urlenc_encodes(input: &str, expected: &str) {
        assert_eq!(urlenc(input), expected);
    }

    struct NeverReader;

    impl futures_lite::io::AsyncRead for NeverReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Pending
        }
    }

    impl futures_lite::io::AsyncBufRead for NeverReader {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<&[u8]>> {
            std::task::Poll::Pending
        }

        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }

    #[test]
    fn next_sse_line_expired_deadline_returns_timeout() {
        smol::block_on(async {
            let mut lines = NeverReader.lines();
            let mut past = Instant::now() - Duration::from_secs(1);
            let stream_timeout = Duration::from_secs(300);
            let err = next_sse_line(&mut lines, &mut past, stream_timeout)
                .await
                .unwrap_err();
            assert!(matches!(err, AgentError::Timeout { .. }));
        })
    }

    #[test]
    fn key_pool_single_key_current() {
        let pool = KeyPool::from_keys(vec!["sk-1".into()]);
        assert_eq!(pool.current(), "sk-1");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn key_pool_single_key_rotate_returns_false() {
        let pool = KeyPool::from_keys(vec!["sk-1".into()]);
        assert!(!pool.rotate());
        assert_eq!(pool.current(), "sk-1");
    }

    #[test]
    fn key_pool_multi_key_rotates() {
        let pool = KeyPool::from_keys(vec!["sk-1".into(), "sk-2".into(), "sk-3".into()]);
        assert_eq!(pool.current(), "sk-1");
        assert!(pool.rotate());
        assert_eq!(pool.current(), "sk-2");
        assert!(pool.rotate());
        assert_eq!(pool.current(), "sk-3");
    }

    #[test]
    fn key_pool_wraps_around() {
        let pool = KeyPool::from_keys(vec!["a".into(), "b".into()]);
        pool.rotate();
        pool.rotate();
        assert_eq!(pool.current(), "a");
    }

    #[test]
    fn resolve_from_env() {
        let env_var = format!("MAKI_TEST_KEY_{}", fastrand::u32(..));
        unsafe { std::env::set_var(&env_var, "from-env") };
        let pool = KeyPool::resolve("test_slug", &env_var).unwrap();
        unsafe { std::env::remove_var(&env_var) };
        assert_eq!(pool.current(), "from-env");
    }

    #[test]
    fn resolve_env_supports_comma_separated() {
        let env_var = format!("MAKI_TEST_MULTI_{}", fastrand::u32(..));
        unsafe { std::env::set_var(&env_var, "sk-1, sk-2, sk-3") };
        let pool = KeyPool::resolve("test_slug", &env_var).unwrap();
        unsafe { std::env::remove_var(&env_var) };
        assert_eq!(pool.current(), "sk-1");
        assert!(pool.rotate());
        assert_eq!(pool.current(), "sk-2");
    }

    #[test]
    fn resolve_returns_error_when_nothing_found() {
        let slug = format!("test_resolve_none_{}", fastrand::u32(..));
        let env_var = format!("MAKI_TEST_KEY_NONE_{}", fastrand::u32(..));
        let result = KeyPool::resolve(&slug, &env_var);
        assert!(result.is_err());
        let msg = format!("{result:?}");
        assert!(msg.contains(&env_var) || msg.contains(&slug));
    }

    #[test]
    fn test_logging_body_accumulates_and_flushes() {
        use std::fs;
        use futures_lite::io::AsyncReadExt;

        let tmp = tempfile::tempdir().unwrap();
        let log_file = tmp.path().join("api.log");

        let input_data = b"hello world streaming data";
        let reader = futures_lite::io::Cursor::new(input_data);

        let logged_body = LoggingBody {
            inner: reader,
            file_path: log_file.clone(),
            status: isahc::http::StatusCode::OK,
            content_type: None,
            accumulated_body: Vec::new(),
            flushed: false,
        };

        let mut buf = Vec::new();
        smol::block_on(async {
            let mut logged_body = logged_body;
            logged_body.read_to_end(&mut buf).await.unwrap();
            drop(logged_body);
        });

        assert_eq!(buf, input_data);
        assert!(log_file.exists());
        let compressed_content = fs::read(log_file).unwrap();
        let log_content = String::from_utf8(zstd::decode_all(&compressed_content[..]).unwrap()).unwrap();
        let log_val: serde_json::Value = serde_json::from_str(&log_content).unwrap();
        assert_eq!(log_val["type"], "response");
        assert_eq!(log_val["status"], 200);
        assert_eq!(log_val["body"], "hello world streaming data");
    }
}
