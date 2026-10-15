use std::collections::BTreeMap;
use std::fmt;

use jiff::Timestamp;

use crate::events::{Event, EventKind};
use crate::model::Session;

const BYTES_PER_TOKEN: usize = 4;
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M";

pub fn approx_tokens(bytes: usize) -> usize {
    bytes.div_ceil(BYTES_PER_TOKEN)
}

#[derive(Debug, Default)]
pub struct Bucket {
    pub count: usize,
    pub bytes: usize,
}

impl Bucket {
    fn add(&mut self, bytes: usize) {
        self.count += 1;
        self.bytes += bytes;
    }

    fn merge(&mut self, other: &Self) {
        self.count += other.count;
        self.bytes += other.bytes;
    }
}

#[derive(Debug, Default)]
pub struct ToolStat {
    pub calls: usize,
    pub errors: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
}

impl ToolStat {
    fn total_bytes(&self) -> usize {
        self.input_bytes + self.output_bytes
    }
}

#[derive(Debug, Default)]
pub struct Stats {
    pub sessions: usize,
    pub messages: usize,
    pub skipped_lines: usize,
    pub prompts: Bucket,
    pub replies: Bucket,
    pub thinking: Bucket,
    pub tools: BTreeMap<String, ToolStat>,
}

impl Stats {
    pub fn add(&mut self, session: &Session, events: &[Event]) {
        self.sessions += 1;
        self.messages += session.turns.len();
        self.skipped_lines += session.skipped_lines;
        self.add_events(events);
    }

    pub fn merge(&mut self, other: Self) {
        self.sessions += other.sessions;
        self.messages += other.messages;
        self.skipped_lines += other.skipped_lines;
        self.prompts.merge(&other.prompts);
        self.replies.merge(&other.replies);
        self.thinking.merge(&other.thinking);
        for (name, stat) in other.tools {
            let mine = self.tools.entry(name).or_default();
            mine.calls += stat.calls;
            mine.errors += stat.errors;
            mine.input_bytes += stat.input_bytes;
            mine.output_bytes += stat.output_bytes;
        }
    }

    fn add_events(&mut self, events: &[Event]) {
        for event in events {
            match &event.kind {
                EventKind::Prompt(text) => self.prompts.add(text.len()),
                EventKind::Reply(text) => self.replies.add(text.len()),
                EventKind::Thinking(text) => self.thinking.add(text.len()),
                EventKind::Image => {}
                EventKind::Detached(subagent) => self.add_events(&subagent.events),
                EventKind::Call(call) => {
                    let stat = self.tools.entry(call.name.to_owned()).or_default();
                    stat.calls += 1;
                    stat.errors += usize::from(call.is_error);
                    stat.input_bytes += call.input.to_string().len();
                    stat.output_bytes += call.output.map_or(0, str::len);
                    if let Some(subagent) = &call.subagent {
                        self.add_events(&subagent.events);
                    }
                }
            }
        }
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(
            f,
            "sessions {}  messages {}  skipped-lines {}",
            self.sessions, self.messages, self.skipped_lines
        )?;
        writeln!(
            f,
            "{:<16}{:>8}{:>12}{:>10}",
            "kind", "n", "bytes", "~tokens"
        )?;
        for (label, bucket) in [
            ("prompt", &self.prompts),
            ("reply", &self.replies),
            ("thinking", &self.thinking),
        ] {
            writeln!(
                f,
                "{label:<16}{:>8}{:>12}{:>10}",
                bucket.count,
                bucket.bytes,
                approx_tokens(bucket.bytes)
            )?;
        }

        let mut tools: Vec<_> = self.tools.iter().collect();
        tools.sort_by_key(|(_, stat)| std::cmp::Reverse(stat.total_bytes()));
        writeln!(
            f,
            "{:<16}{:>8}{:>8}{:>12}{:>12}{:>10}",
            "tool", "calls", "errors", "in-bytes", "out-bytes", "~tokens"
        )?;
        for (name, stat) in tools {
            writeln!(
                f,
                "{name:<16}{:>8}{:>8}{:>12}{:>12}{:>10}",
                stat.calls,
                stat.errors,
                stat.input_bytes,
                stat.output_bytes,
                approx_tokens(stat.total_bytes())
            )?;
        }
        Ok(())
    }
}

pub fn summary_line(session: &Session, events: &[Event], file_bytes: usize) -> String {
    let date = Timestamp::from_second(session.header.created_at)
        .map(|timestamp| timestamp.strftime(DATE_FORMAT).to_string())
        .unwrap_or_default();
    let calls = events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::Call(_)))
        .count();
    let header = &session.header;
    format!(
        "{date}\t{}\t{}\t{calls}\t{file_bytes}\t{}\t{}\t{}",
        header.id,
        session.turns.len(),
        header.model,
        header.cwd,
        session.title()
    )
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::events::build;
    use crate::model::ParseOpts;

    const LOG: &str = concat!(
        r#"{"t":"header","v":2,"id":"sid","model":"m","cwd":"/w","created_at":1790625106}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"tool_use","id":"a","name":"bash","input":{"command":"ls"}},{"type":"tool_use","id":"b","name":"bash","input":{"command":"x"}}]}}"#,
        "\n",
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"12345678"},{"type":"tool_result","tool_use_id":"b","content":"boom","is_error":true}]}}"#,
        "\n",
        r#"{"t":"meta","title":"T","updated_at":0}"#,
    );

    #[test_case(0, 0; "empty")]
    #[test_case(1, 1; "rounds_up")]
    #[test_case(8, 2; "exact")]
    fn approximates_tokens_from_bytes(bytes: usize, expected: usize) {
        assert_eq!(approx_tokens(bytes), expected);
    }

    #[test]
    fn aggregates_calls_errors_and_bytes_per_tool() {
        let session = Session::parse(LOG, ParseOpts::default()).unwrap();
        let mut stats = Stats::default();
        stats.add(&session, &build(&session));
        let bash = &stats.tools["bash"];
        assert_eq!((bash.calls, bash.errors, bash.output_bytes), (2, 1, 12));
    }

    #[test]
    fn merge_sums_every_counter() {
        let session = Session::parse(LOG, ParseOpts::default()).unwrap();
        let mut a = Stats::default();
        a.add(&session, &build(&session));
        let mut b = Stats::default();
        b.add(&session, &build(&session));
        a.merge(b);
        assert_eq!((a.sessions, a.tools["bash"].calls), (2, 4));
    }

    #[test]
    fn summary_line_is_tab_separated_with_title_last() {
        let session = Session::parse(LOG, ParseOpts::default()).unwrap();
        let line = summary_line(&session, &build(&session), 99);
        assert_eq!(line, "2026-09-28 19:51\tsid\t2\t2\t99\tm\t/w\tT");
    }
}
