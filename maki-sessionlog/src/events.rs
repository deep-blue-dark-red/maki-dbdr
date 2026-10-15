use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::model::{Block, Diff, Role, Session, SubagentInfo, Turn};

const DETACHED_LABEL: &str = "detached";

#[derive(Debug)]
pub struct Event<'a> {
    pub at: usize,
    pub kind: EventKind<'a>,
}

#[derive(Debug)]
pub enum EventKind<'a> {
    Prompt(&'a str),
    Reply(&'a str),
    Thinking(&'a str),
    Image,
    Call(Call<'a>),
    /// A subagent transcript whose parent call is gone from the log, usually
    /// removed by compaction or a rewind. Its position in time is unknown, so
    /// it is placed after the last message.
    Detached(Subagent<'a>),
}

#[derive(Debug)]
pub struct Call<'a> {
    pub name: &'a str,
    pub input: &'a Value,
    pub output: Option<&'a str>,
    pub is_error: bool,
    pub diff: Option<&'a Diff>,
    pub subagent: Option<Subagent<'a>>,
}

#[derive(Debug)]
pub struct Subagent<'a> {
    pub name: Option<&'a str>,
    pub model: Option<&'a str>,
    pub events: Vec<Event<'a>>,
}

impl EventKind<'_> {
    pub fn role(&self) -> Role {
        match self {
            Self::Prompt(_) | Self::Image => Role::User,
            Self::Reply(_) | Self::Thinking(_) | Self::Call(_) | Self::Detached(_) => {
                Role::Assistant
            }
        }
    }
}

pub fn build<'a>(session: &'a Session<'_>) -> Vec<Event<'a>> {
    let subagents = session
        .meta
        .as_ref()
        .map_or(&[][..], |meta| meta.subagents.as_slice());
    let builder = Builder { session, subagents };
    let mut events = builder.events(&session.turns);

    let known_calls = known_call_ids(session);
    for key in &session.subagent_order {
        if known_calls.contains(&**key) {
            continue;
        }
        let mut subagent = builder.subagent_of(key, &session.subagent_turns[key]);
        subagent.name = subagent.name.or(Some(DETACHED_LABEL));
        events.push(Event {
            at: session.turns.len(),
            kind: EventKind::Detached(subagent),
        });
    }
    events
}

fn known_call_ids<'a>(session: &'a Session<'_>) -> HashSet<&'a str> {
    session
        .turns
        .iter()
        .chain(session.subagent_turns.values().flatten())
        .flat_map(|turn| &turn.content)
        .filter_map(|block| match block {
            Block::ToolUse { id, .. } => Some(&**id),
            _ => None,
        })
        .collect()
}

struct Builder<'a> {
    session: &'a Session<'a>,
    subagents: &'a [SubagentInfo<'a>],
}

impl<'a> Builder<'a> {
    fn events(&self, turns: &'a [Turn<'a>]) -> Vec<Event<'a>> {
        let everywhere = collect_results(turns);
        let mut events = Vec::new();
        for (at, turn) in turns.iter().enumerate() {
            let next_turn = collect_results(turns.get(at + 1..at + 2).unwrap_or_default());
            for block in &turn.content {
                let kind = match (turn.role, block) {
                    (Role::User, Block::Text { text }) => EventKind::Prompt(text),
                    (Role::Assistant, Block::Text { text }) => EventKind::Reply(text),
                    (_, Block::Thinking { thinking }) => EventKind::Thinking(thinking),
                    (_, Block::Image {}) => EventKind::Image,
                    (_, Block::ToolUse { id, name, input }) => {
                        let result = next_turn.get(&**id).or_else(|| everywhere.get(&**id));
                        EventKind::Call(Call {
                            name,
                            input,
                            output: result.map(|(content, _)| *content),
                            is_error: result.is_some_and(|(_, is_error)| *is_error),
                            diff: self.session.diffs.get(&**id),
                            subagent: self.subagent(id),
                        })
                    }
                    _ => continue,
                };
                events.push(Event { at, kind });
            }
        }
        events
    }

    fn subagent(&self, call_id: &str) -> Option<Subagent<'a>> {
        let turns = self.session.subagent_turns.get(call_id)?;
        Some(self.subagent_of(call_id, turns))
    }

    fn subagent_of(&self, call_id: &str, turns: &'a [Turn<'a>]) -> Subagent<'a> {
        let info = self
            .subagents
            .iter()
            .find(|info| info.tool_use_id == call_id);
        Subagent {
            name: info.map(|info| &*info.name),
            model: info.and_then(|info| info.model.as_deref()),
            events: self.events(turns),
        }
    }
}

/// Providers sometimes reuse a call id across turns, so a call is first matched
/// against the user message that directly follows it and only then against the
/// whole transcript.
fn collect_results<'a>(turns: &'a [Turn<'a>]) -> HashMap<&'a str, (&'a str, bool)> {
    turns
        .iter()
        .flat_map(|turn| &turn.content)
        .filter_map(|block| match block {
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((&**tool_use_id, (&**content, *is_error))),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ParseOpts;

    const LOG: &str = concat!(
        r#"{"t":"header","v":2,"id":"i","model":"m","cwd":"/","created_at":0}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"text","text":"go"}]}}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"tool_use","id":"c1","name":"task","input":{"prompt":"p"}}]}}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"done","is_error":true}]}}"#,
        "\n",
        r#"{"t":"sub_msg","sub":"c1","d":{"role":"assistant","content":[{"type":"text","text":"inner"}]}}"#,
        "\n",
        r#"{"t":"meta","title":"t","updated_at":0,"subagents":[{"tool_use_id":"c1","name":"scout","model":"fast"}]}"#,
    );

    #[test]
    fn pairs_calls_with_results_and_nests_subagents() {
        let session = Session::parse(LOG, ParseOpts::default()).unwrap();
        let events = build(&session);

        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].kind, EventKind::Prompt("go")));
        assert!(matches!(events[1].kind, EventKind::Thinking("hm")));
        let EventKind::Call(call) = &events[2].kind else {
            panic!("expected a call");
        };
        assert_eq!(call.output, Some("done"));
        assert!(call.is_error);
        let subagent = call.subagent.as_ref().unwrap();
        assert_eq!(subagent.name, Some("scout"));
        assert_eq!(subagent.model, Some("fast"));
        assert!(matches!(subagent.events[0].kind, EventKind::Reply("inner")));
    }

    #[test]
    fn events_remember_their_message_index() {
        let session = Session::parse(LOG, ParseOpts::default()).unwrap();
        let positions: Vec<_> = build(&session).iter().map(|event| event.at).collect();
        assert_eq!(positions, [0, 1, 1]);
    }

    #[test]
    fn subagent_transcripts_with_a_missing_parent_call_become_detached_events() {
        let text = concat!(
            r#"{"t":"header","v":2,"id":"i","model":"m","cwd":"/","created_at":0}"#,
            "\n",
            r#"{"t":"msg","d":{"role":"user","content":[{"type":"text","text":"go"}]}}"#,
            "\n",
            r#"{"t":"sub_msg","sub":"gone","d":{"role":"assistant","content":[{"type":"tool_use","id":"s1","name":"bash","input":{}},{"type":"tool_use","id":"s2","name":"task","input":{}}]}}"#,
            "\n",
            r#"{"t":"sub_msg","sub":"s2","d":{"role":"assistant","content":[{"type":"text","text":"nested"}]}}"#,
        );
        let session = Session::parse(text, ParseOpts::default()).unwrap();
        let events = build(&session);

        assert_eq!(events.len(), 2);
        let EventKind::Detached(subagent) = &events[1].kind else {
            panic!("expected a detached subagent");
        };
        assert_eq!(subagent.name, Some("detached"));
        assert_eq!(subagent.events.len(), 2);
        let EventKind::Call(nested_parent) = &subagent.events[1].kind else {
            panic!("expected the nested task call");
        };
        assert!(
            nested_parent.subagent.is_some(),
            "nested groups must not be detached twice"
        );
    }

    #[test]
    fn a_reused_call_id_takes_the_result_from_its_own_turn() {
        let call = |text: &str| {
            format!(
                r#"{{"t":"msg","d":{{"role":"assistant","content":[{{"type":"tool_use","id":"dup","name":"bash","input":{{"command":"{text}"}}}}]}}}}"#
            )
        };
        let result = |content: &str| {
            format!(
                r#"{{"t":"msg","d":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"dup","content":"{content}"}}]}}}}"#
            )
        };
        let text = [
            r#"{"t":"header","v":2,"id":"i","model":"m","cwd":"/","created_at":0}"#.to_owned(),
            call("first"),
            result("one"),
            call("second"),
            result("two"),
        ]
        .join("\n");
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        let outputs: Vec<_> = build(&session)
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::Call(call) => call.output,
                _ => None,
            })
            .collect();

        assert_eq!(outputs, ["one", "two"]);
    }
}
