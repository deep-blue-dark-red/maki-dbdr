use std::ops::RangeInclusive;

use regex::Regex;

use crate::events::{Call, Event, EventKind};
use crate::model::Role;

#[derive(Default)]
pub struct Filter {
    pub role: Option<Role>,
    pub tools: Vec<String>,
    pub errors_only: bool,
    pub grep: Option<Regex>,
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub last: Option<usize>,
    pub no_thinking: bool,
    pub no_tools: bool,
    pub no_subagents: bool,
}

impl Filter {
    pub fn apply<'a>(&self, events: Vec<Event<'a>>) -> Vec<Event<'a>> {
        let window = self.window(&events);
        self.retain(events, &window)
    }

    fn window(&self, events: &[Event]) -> RangeInclusive<usize> {
        let message_count = events.last().map_or(0, |event| event.at + 1);
        let from_last = self
            .last
            .map_or(0, |last| message_count.saturating_sub(last));
        from_last.max(self.from.unwrap_or(0))..=self.to.unwrap_or(usize::MAX)
    }

    fn retain<'a>(
        &self,
        mut events: Vec<Event<'a>>,
        window: &RangeInclusive<usize>,
    ) -> Vec<Event<'a>> {
        events.retain_mut(|event| self.keep(event, window));
        events
    }

    fn keep(&self, event: &mut Event, window: &RangeInclusive<usize>) -> bool {
        if !window.contains(&event.at) {
            return false;
        }
        if self.role.is_some_and(|role| event.kind.role() != role) {
            return false;
        }
        match &mut event.kind {
            EventKind::Thinking(_) => {
                !self.no_thinking && !self.errors_only && self.matches_text(&event.kind)
            }
            EventKind::Call(call) => !self.no_tools && self.keep_call(call),
            EventKind::Detached(subagent) => {
                if self.no_subagents {
                    return false;
                }
                subagent.events =
                    self.retain(std::mem::take(&mut subagent.events), &(0..=usize::MAX));
                !subagent.events.is_empty()
            }
            kind => !self.errors_only && self.matches_text(kind),
        }
    }

    fn keep_call(&self, call: &mut Call) -> bool {
        if self.no_subagents {
            call.subagent = None;
        } else if let Some(subagent) = &mut call.subagent {
            subagent.events = self.retain(std::mem::take(&mut subagent.events), &(0..=usize::MAX));
        }
        let tool_listed = self.tools.is_empty() || self.tools.iter().any(|tool| tool == call.name);
        let errors_ok = !self.errors_only || call.is_error;
        tool_listed
            && errors_ok
            && self
                .grep
                .as_ref()
                .is_none_or(|grep| call_matches(grep, call))
    }

    fn matches_text(&self, kind: &EventKind) -> bool {
        let text = match kind {
            EventKind::Prompt(text) | EventKind::Reply(text) | EventKind::Thinking(text) => text,
            _ => return self.grep.is_none(),
        };
        self.grep.as_ref().is_none_or(|grep| grep.is_match(text))
    }
}

fn call_matches(grep: &Regex, call: &Call) -> bool {
    grep.is_match(call.name)
        || grep.is_match(&call.input.to_string())
        || call.output.is_some_and(|output| grep.is_match(output))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use test_case::test_case;

    use super::*;
    use crate::events::Subagent;

    fn event(at: usize, kind: EventKind) -> Event {
        Event { at, kind }
    }

    fn call<'a>(name: &'a str, input: &'a Value, output: &'a str, is_error: bool) -> EventKind<'a> {
        EventKind::Call(Call {
            name,
            input,
            output: Some(output),
            is_error,
            diff: None,
            subagent: None,
        })
    }

    fn kinds(events: &[Event]) -> Vec<&'static str> {
        events
            .iter()
            .map(|event| match event.kind {
                EventKind::Prompt(_) => "prompt",
                EventKind::Reply(_) => "reply",
                EventKind::Thinking(_) => "thinking",
                EventKind::Image => "image",
                EventKind::Call(_) => "call",
                EventKind::Detached(_) => "detached",
            })
            .collect()
    }

    fn sample(input: &Value) -> Vec<Event<'_>> {
        vec![
            event(0, EventKind::Prompt("fix the bug")),
            event(1, EventKind::Thinking("looking")),
            event(1, call("bash", input, "ok", false)),
            event(2, call("read", input, "panic: boom", true)),
            event(3, EventKind::Reply("done")),
        ]
    }

    #[test]
    fn default_filter_keeps_everything() {
        let input = json!({});
        let kept = Filter::default().apply(sample(&input));
        assert_eq!(kept.len(), 5);
    }

    #[test_case(Filter { no_thinking: true, ..Filter::default() }, &["prompt", "call", "call", "reply"]; "no_thinking")]
    #[test_case(Filter { no_tools: true, ..Filter::default() }, &["prompt", "thinking", "reply"]; "no_tools")]
    #[test_case(Filter { errors_only: true, ..Filter::default() }, &["call"]; "errors_only")]
    #[test_case(Filter { role: Some(Role::User), ..Filter::default() }, &["prompt"]; "user_only")]
    #[test_case(Filter { tools: vec!["read".into()], ..Filter::default() }, &["prompt", "thinking", "call", "reply"]; "tool_allowlist")]
    #[test_case(Filter { last: Some(2), ..Filter::default() }, &["call", "reply"]; "last_messages")]
    #[test_case(Filter { from: Some(1), to: Some(1), ..Filter::default() }, &["thinking", "call"]; "message_range")]
    fn selects_events(filter: Filter, expected: &[&str]) {
        let input = json!({});
        assert_eq!(kinds(&filter.apply(sample(&input))), expected);
    }

    #[test]
    fn grep_matches_call_output_and_text() {
        let input = json!({});
        let filter = Filter {
            grep: Some(Regex::new("boom|fix").unwrap()),
            ..Filter::default()
        };
        assert_eq!(kinds(&filter.apply(sample(&input))), ["prompt", "call"]);
    }

    #[test]
    fn detached_subagents_follow_the_subagent_and_content_filters() {
        let input = json!({});
        let detached = || {
            event(
                1,
                EventKind::Detached(Subagent {
                    name: Some("detached"),
                    model: None,
                    events: vec![
                        event(0, EventKind::Thinking("hm")),
                        event(0, call("bash", &input, "ok", false)),
                    ],
                }),
            )
        };

        let no_subagents = Filter {
            no_subagents: true,
            ..Filter::default()
        };
        assert!(no_subagents.apply(vec![detached()]).is_empty());

        let no_tools = Filter {
            no_tools: true,
            ..Filter::default()
        };
        let kept = no_tools.apply(vec![detached()]);
        let EventKind::Detached(subagent) = &kept[0].kind else {
            panic!("expected the detached subagent to stay");
        };
        assert_eq!(kinds(&subagent.events), ["thinking"]);

        let nothing_left = Filter {
            no_tools: true,
            no_thinking: true,
            ..Filter::default()
        };
        assert!(nothing_left.apply(vec![detached()]).is_empty());
    }
}
