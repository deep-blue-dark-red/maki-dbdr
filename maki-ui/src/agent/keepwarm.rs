use std::time::Duration;

use maki_providers::Message;

/// Well under the ~5 min TTL providers give an untouched prompt cache, so a
/// hit arrives while the entry is still refreshable.
pub(super) const WARM_INTERVAL: Duration = Duration::from_secs(30);

/// Below this a cold prefill costs about as much as keeping the entry warm.
const WARM_MIN_PROMPT_TOKENS: u32 = 8_192;

/// Pings allowed per idle stretch; a completed run resets the count. Bounds
/// spend when a session sits open unattended.
pub(crate) const WARM_MAX_PINGS: u32 = 20;

/// Consecutive misses before the stretch stands down. The first miss after a
/// busy stretch only means the TTL lapsed; the next ping rewrites the cache
/// and the one after hits.
pub(crate) const WARM_MAX_MISSES: u32 = 2;

const WARM_PROMPT: &str = "[keep-alive] Automated cache refresh, not a task. Reply with: ok";

/// What one `keep_warm` attempt did.
pub(super) enum WarmOutcome {
    /// A request went out; the value is what it read back from the cache,
    /// `0` meaning the provider served it cold.
    Sent(u32),
    /// A request went out but no usage came back. It still counts toward the
    /// budget, so a dead provider cannot ping forever.
    Failed,
    /// Nothing went out this stretch: stand down until the next run.
    StandDown,
}

/// Whether the idle timer may arm another ping: keep-warm on, budget left,
/// and nothing that stood the stretch down.
pub(super) fn warm_scheduled(enabled: bool, pings: u32, stop: bool) -> bool {
    enabled && !stop && pings < WARM_MAX_PINGS
}

pub(super) fn warm_due(prompt_tokens: u32) -> bool {
    prompt_tokens >= WARM_MIN_PROMPT_TOKENS
}

/// The ping replays the conversation verbatim plus one user turn (providers
/// require the last message to be a user turn). The extra turn never enters
/// the real history, so the next real request's prefix - the part the cache
/// actually holds - matches what this request read.
pub(super) fn warm_messages(snapshot: &[Message]) -> Vec<Message> {
    let mut messages = Vec::with_capacity(snapshot.len() + 1);
    messages.extend_from_slice(snapshot);
    messages.push(Message::user(WARM_PROMPT.into()));
    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_messages_appends_the_ping_without_touching_the_snapshot() {
        let snapshot = vec![Message::user("real prompt".into())];
        let messages = warm_messages(&snapshot);
        assert_eq!(messages.len(), 2);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(messages[0].first_text_content(), Some("real prompt"));
        assert_eq!(
            messages.last().map(Message::first_text_content),
            Some(Some(WARM_PROMPT))
        );
    }

    #[test]
    fn warm_due_skips_small_prompts() {
        assert!(!warm_due(WARM_MIN_PROMPT_TOKENS - 1));
        assert!(warm_due(WARM_MIN_PROMPT_TOKENS));
    }

    #[test]
    fn warm_scheduled_stops_on_spent_budget_miss_or_disabled() {
        assert!(warm_scheduled(true, 0, false));
        assert!(warm_scheduled(true, WARM_MAX_PINGS - 1, false));
        assert!(!warm_scheduled(true, WARM_MAX_PINGS, false), "budget spent");
        assert!(!warm_scheduled(true, 0, true), "last ping missed");
        assert!(!warm_scheduled(false, 0, false), "keep-warm off");
    }
}
