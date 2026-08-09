use std::time::Instant;

use maki_providers::provider::Provider;
use maki_providers::retry::{MAX_TIMEOUT_RETRIES, RetryState};
use maki_providers::{Message, Model, ProviderEvent, RequestOptions, StreamResponse};
use maki_storage::id::SessionRef;
use serde_json::Value;
use tracing::warn;

use crate::cancel::CancelToken;
use crate::{AgentError, AgentEvent, EventSender};

/// Forwards provider events to the UI, returning when the first actual
/// response content arrived (excluding `PromptProgress`, which reports
/// upload progress of the *request*, not the start of the response).
async fn forward_provider_events(
    prx: flume::Receiver<ProviderEvent>,
    event_tx: &EventSender,
) -> Option<Instant> {
    let mut first_byte_at = None;
    while let Ok(pe) = prx.recv_async().await {
        let ae = match pe {
            ProviderEvent::TextDelta { text } => AgentEvent::TextDelta { text },
            ProviderEvent::ThinkingDelta { text } => AgentEvent::ThinkingDelta { text },
            ProviderEvent::ToolUseStart { id, name } => AgentEvent::ToolPending { id, name },
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
    first_byte_at
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
) -> Result<StreamResponse, AgentError> {
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
        if let Some(at) = forwarder.await {
            first_byte_at.get_or_insert(at);
        }
        match result {
            Ok(r) => return Ok(r),
            Err(AgentError::Cancelled) => return Err(AgentError::Cancelled),
            Err(e) if e.is_retryable() => {
                if e.should_rotate_key()
                    && let Ok(true) = provider.rotate_key().await
                {
                    warn!("rotated API key after error: {e}");
                }
                let (attempt, delay) = retry.next_delay();
                *api_error_count = attempt;
                if matches!(e, AgentError::Timeout { .. }) && attempt > MAX_TIMEOUT_RETRIES {
                    return Err(e);
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
                    return Err(AgentError::Cancelled);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

pub(crate) fn estimate_input_tokens(messages: &[Message], system: &str, tools: &Value) -> u32 {
    let mut total_bytes = system.len();
    if !tools.is_null() {
        total_bytes += tools.to_string().len();
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
                    total_bytes += input.to_string().len();
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

