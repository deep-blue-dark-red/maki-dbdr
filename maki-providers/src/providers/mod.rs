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

fn now_ms() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

/// Path of the current session's `.mlog` file, keyed by the stable session id so
/// every turn of a session lands in one file. `None` if logs are unavailable or
/// there's no active session.
fn log_file_path() -> Option<std::path::PathBuf> {
    let logs_dir = maki_storage::paths::logs_dir().ok()?;
    let session_id = maki_config::CURRENT_SESSION_ID
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|id| !id.is_empty())?;
    Some(logs_dir.join(format!("{session_id}.mlog")))
}

/// The `YYYYMMDD-<title>.mlog` symlink path for a session title, dated to the
/// session's creation (`created_at`, Unix epoch seconds) so the link is stable
/// across renames on later days. `None` for a placeholder/blank title.
fn friendly_log_link(
    logs_dir: &std::path::Path,
    name: &str,
    created_at: u64,
) -> Option<std::path::PathBuf> {
    if name.is_empty() || name == "Main" {
        return None;
    }
    let sanitized: String = name
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                Some(c)
            } else if c.is_whitespace() {
                Some('_')
            } else {
                None
            }
        })
        .collect();
    if sanitized.is_empty() {
        return None;
    }
    let yyyymmdd = jiff::Timestamp::from_second(created_at as i64)
        .unwrap_or_else(|_| jiff::Timestamp::now())
        .to_string()[..10]
        .replace('-', "");
    Some(logs_dir.join(format!("{yyyymmdd}-{sanitized}.mlog")))
}

/// Remove `path` only if it is a symlink, never a real log file.
fn remove_if_symlink(path: &std::path::Path) {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = std::fs::remove_file(path);
    }
}

/// Maintain a friendly-named symlink (`YYYYMMDD-<title>.mlog`, dated to session
/// creation) pointing at a session's canonical id-based log, so the logs dir is
/// browsable by title while the real file stays keyed by the stable session id.
/// Best-effort: drops a stale link from the previous title, and skips silently
/// if nothing has been logged yet (e.g. API logging disabled), on collision with
/// a real file, or on FS error.
pub fn update_api_log_symlink(
    session_id: &str,
    old_name: Option<&str>,
    new_name: &str,
    created_at: u64,
) {
    let Ok(logs_dir) = maki_storage::paths::logs_dir() else {
        return;
    };

    if let Some(old) = old_name.and_then(|n| friendly_log_link(&logs_dir, n, created_at)) {
        remove_if_symlink(&old);
    }

    let target = logs_dir.join(format!("{session_id}.mlog"));
    if !target.exists() {
        return; // nothing logged for this session yet
    }
    let Some(link) = friendly_log_link(&logs_dir, new_name, created_at) else {
        return;
    };
    if link == target {
        return;
    }
    remove_if_symlink(&link);
    // Relative target so the link survives moving the logs directory.
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(format!("{session_id}.mlog"), &link);
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
    send_request_logged(client, request, None).await
}

/// Like [`send_request`], but with per-message wire [`Fragments`] for the log so
/// the request body is deduplicated at message granularity. Fragments are only
/// used if they reproduce the exact bytes; otherwise the logger byte-diffs.
pub(crate) async fn send_request_with_fragments(
    client: &isahc::HttpClient,
    request: isahc::Request<Vec<u8>>,
    fragments: Option<crate::wire_log::Fragments>,
) -> Result<isahc::Response<isahc::AsyncBody>, AgentError> {
    send_request_logged(client, request, fragments).await
}

async fn send_request_logged(
    client: &isahc::HttpClient,
    request: isahc::Request<Vec<u8>>,
    fragments: Option<crate::wire_log::Fragments>,
) -> Result<isahc::Response<isahc::AsyncBody>, AgentError> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();

    let file_path = (maki_config::LOG_API.load(Ordering::Relaxed)
        && is_chat_completion_request(&method, &uri))
    .then(log_file_path)
    .flatten();

    let Some(file_path) = file_path else {
        let (parts, body) = request.into_parts();
        let req = isahc::Request::from_parts(parts, isahc::AsyncBody::from(body));
        return client.send_async(req).await.map_err(Into::into);
    };

    crate::wire_log::log_request(&file_path, now_ms(), &uri, request.body(), fragments);

    let (parts, body) = request.into_parts();
    let req = isahc::Request::from_parts(parts, isahc::AsyncBody::from(body));
    let response = client.send_async(req).await?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();

    let (res_parts, res_body) = response.into_parts();
    let logged_body = LoggingBody {
        inner: res_body,
        file_path,
        status,
        content_type,
        accumulated_body: Vec::new(),
        flushed: false,
    };
    Ok(isahc::Response::from_parts(
        res_parts,
        isahc::AsyncBody::from_reader(logged_body),
    ))
}

struct LoggingBody<R> {
    inner: R,
    file_path: std::path::PathBuf,
    status: u16,
    content_type: String,
    accumulated_body: Vec<u8>,
    flushed: bool,
}

impl<R> LoggingBody<R> {
    fn flush_log(&mut self) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        crate::wire_log::log_response(
            &self.file_path,
            now_ms(),
            self.status,
            &self.content_type,
            &self.accumulated_body,
        );
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
        use crate::wire_log::{self, Record};
        use futures_lite::io::AsyncReadExt;

        let tmp = tempfile::tempdir().unwrap();
        let log_file = tmp.path().join("api.mlog");

        let input_data = b"hello world streaming data";
        let reader = futures_lite::io::Cursor::new(input_data);

        let logged_body = LoggingBody {
            inner: reader,
            file_path: log_file.clone(),
            status: 200,
            content_type: "application/json".to_string(),
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

        let records = wire_log::read_file(&log_file).unwrap();
        let Some(Record::Response { status, body, .. }) = records.first() else {
            panic!("expected a response record");
        };
        assert_eq!(*status, 200);
        assert_eq!(body, input_data);
    }
}
