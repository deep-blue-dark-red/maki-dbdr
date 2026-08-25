use std::time::Instant;

use maki_providers::provider::Provider;
use maki_providers::retry::{MAX_TIMEOUT_RETRIES, RetryState};
use maki_providers::{ContentBlock, Message, Model, ProviderEvent, RequestOptions, StreamResponse};
use maki_storage::id::SessionRef;
use serde_json::Value;
use tracing::warn;

use crate::cancel::CancelToken;
use crate::{AgentError, AgentEvent, EventSender};

const FUNCTIONS_PREFIX: &str = "functions.";

/// GPT models sometimes emit `functions.<name>`, a Codex training habit.
/// Stripped here at the provider boundary so no raw name enters the agent;
/// the batch plugin mirrors the rule in Lua.
pub(crate) fn canonical_tool_name(name: &str) -> &str {
    name.strip_prefix(FUNCTIONS_PREFIX).unwrap_or(name)
}

fn canonicalize_tool_names(message: &mut Message) {
    for block in &mut message.content {
        if let ContentBlock::ToolUse { name, .. } = block {
            *name = canonical_tool_name(name).to_owned();
        }
    }
}

/// Forwards provider events to the UI, returning when the first actual
/// response content arrived (excluding `PromptProgress`, which reports upload
/// progress of the *request*, not the start of the response), plus the text
/// streamed so far so a cancel can keep what the user still sees on screen.
async fn forward_provider_events(
    prx: flume::Receiver<ProviderEvent>,
    event_tx: &EventSender,
) -> (Option<Instant>, String) {
    let mut first_byte_at = None;
    let mut streamed = String::new();
    while let Ok(pe) = prx.recv_async().await {
        let ae = match pe {
            ProviderEvent::TextDelta { text } => {
                streamed.push_str(&text);
                AgentEvent::TextDelta { text }
            }
            ProviderEvent::ThinkingDelta { text } => AgentEvent::ThinkingDelta { text },
            ProviderEvent::ToolUseStart { id, name } => AgentEvent::ToolPending {
                id,
                name: canonical_tool_name(&name).to_owned(),
            },
            ProviderEvent::PromptProgress {
                processed,
                total,
                cache,
            } => {
                let ae = AgentEvent::PromptProgress {
                    processed,
                    total,
                    cache,
                };
                if event_tx.send(ae).is_err() {
                    break;
                }
                continue;
            }
        };
        first_byte_at.get_or_insert_with(Instant::now);
        if event_tx.send(ae).is_err() {
            break;
        }
    }
    (first_byte_at, streamed)
}

/// Cancelling mid-stream carries the text the user still sees on screen,
/// so the caller can keep it in history. A cancel during the retry backoff
/// carries nothing: the `Retry` event already made the view drop the failed
/// attempt's text (`stream_reset`), and history must agree with the view.
#[derive(Debug)]
pub(crate) enum StreamError {
    Cancelled { streamed: String },
    Other(AgentError),
}

impl From<AgentError> for StreamError {
    fn from(e: AgentError) -> Self {
        Self::Other(e)
    }
}

impl From<StreamError> for AgentError {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::Cancelled { .. } => Self::Cancelled,
            StreamError::Other(e) => e,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_with_retry(
    provider: &dyn Provider,
    model: &Model,
    messages: &[Message],
    system: &str,
    tools: &Value,
    event_tx: &EventSender,
    cancel: &CancelToken,
    opts: RequestOptions,
    session_id: Option<&SessionRef>,
    first_byte_at: &mut Option<Instant>,
    api_error_count: &mut u32,
) -> Result<StreamResponse, StreamError> {
    let opts = opts.clamped(model);
    let messages = maki_providers::adapt_images_for_model(model, messages);
    let messages = &*messages;

    let mut model = model.clone();
    let input_tokens = estimate_input_tokens(messages, system, tools);
    let remaining = model
        .context_window
        .saturating_sub(input_tokens)
        .saturating_sub(1000);
    model.max_output_tokens = Some(
        model
            .max_output_tokens
            .map_or(remaining, |max| max.min(remaining))
            .max(1),
    );

    let mut retry = RetryState::new();
    loop {
        let (ptx, prx) = flume::unbounded();
        let forwarder = smol::spawn({
            let event_tx = event_tx.clone();
            async move { forward_provider_events(prx, &event_tx).await }
        });
        let result = futures_lite::future::race(
            provider.stream_message(&model, messages, system, tools, &ptx, opts, session_id),
            async {
                cancel.cancelled().await;
                Err(AgentError::Cancelled)
            },
        )
        .await;
        drop(ptx);
        let (at, streamed) = forwarder.await;
        if let Some(at) = at {
            first_byte_at.get_or_insert(at);
        }
        match result {
            Ok(mut r) => {
                canonicalize_tool_names(&mut r.message);
                return Ok(r);
            }
            Err(AgentError::Cancelled) => return Err(StreamError::Cancelled { streamed }),
            Err(e) if e.is_retryable() => {
                if e.should_rotate_key()
                    && let Ok(true) = provider.rotate_key().await
                {
                    warn!("rotated API key after error: {e}");
                }
                let (attempt, delay) = retry.next_delay();
                *api_error_count = attempt;
                if matches!(e, AgentError::Timeout { .. }) && attempt > MAX_TIMEOUT_RETRIES {
                    return Err(e.into());
                }
                let delay_ms = delay.as_millis() as u64;
                warn!(attempt, delay_ms, error = %e, "retryable, will retry");
                event_tx.send(AgentEvent::Retry {
                    attempt,
                    message: e.retry_message(),
                    delay_ms,
                })?;
                futures_lite::future::race(
                    async {
                        smol::Timer::after(delay).await;
                    },
                    cancel.cancelled(),
                )
                .await;
                if cancel.is_cancelled() {
                    return Err(StreamError::Cancelled {
                        streamed: String::new(),
                    });
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn json_byte_len(val: &Value) -> usize {
    let mut counter = ByteCounter(0);
    let _ = serde_json::to_writer(&mut counter, val);
    counter.0
}

pub(crate) fn estimate_input_tokens(messages: &[Message], system: &str, tools: &Value) -> u32 {
    let mut total_bytes = system.len();
    if !tools.is_null() {
        total_bytes += json_byte_len(tools);
    }
    for m in messages {
        for b in &m.content {
            match b {
                maki_providers::ContentBlock::Text { text } => {
                    total_bytes += text.len();
                }
                maki_providers::ContentBlock::ToolResult { content, .. } => {
                    total_bytes += content.len();
                }
                maki_providers::ContentBlock::ToolUse { input, .. } => {
                    total_bytes += json_byte_len(input);
                }
                maki_providers::ContentBlock::Thinking { thinking, .. } => {
                    total_bytes += thinking.len();
                }
                maki_providers::ContentBlock::RedactedThinking { data } => {
                    total_bytes += data.len();
                }
                _ => {}
            }
        }
    }
    const CHARS_PER_TOKEN: usize = 4;
    (total_bytes.max(CHARS_PER_TOKEN) / CHARS_PER_TOKEN) as u32
}

#[cfg(test)]
mod tests {
    use maki_providers::Role;
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_use_names_canonicalized() {
        let mut message = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text { text: "hi".into() },
                ContentBlock::tool_use("t1", "functions.bash", json!({})),
                ContentBlock::tool_use("t2", "read", json!({})),
                ContentBlock::tool_use("t3", "my_functions.x", json!({})),
            ],
            ..Default::default()
        };
        canonicalize_tool_names(&mut message);
        let names: Vec<&str> = message.tool_uses().map(|(_, name, _)| name).collect();
        assert_eq!(names, ["bash", "read", "my_functions.x"]);
    }
}
